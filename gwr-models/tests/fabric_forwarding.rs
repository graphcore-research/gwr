// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::traits::{Routable, TotalBytes};
use gwr_engine::types::{AccessType, DeviceId};
use gwr_models::fabric::node::FabricRoutingAlgorithm;
use gwr_models::fabric::paths::FabricPathConfig;
use gwr_models::fabric::routed::RoutedFabric;
use gwr_models::fabric::service::RouteServiceSnapshot;
use gwr_models::fabric::{Fabric, FabricConfig, FabricGeometry, FabricPortConfig};
use gwr_models::memory::memory_access::MemoryAccess;

struct Observed {
    arrivals: Vec<u64>,
    acks: Vec<u64>,
    during: Vec<RouteServiceSnapshot>,
    final_stats: Vec<RouteServiceSnapshot>,
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep complete setup execution and assertions of this scenario together"
)]
fn run(
    columns: usize,
    rate: usize,
    overhead: usize,
    sizes: &[usize],
    stalled_until: u64,
) -> Observed {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let destination = (columns - 1) * 2 + 1;
    let config = FabricConfig::new(
        FabricGeometry {
            num_columns: columns,
            num_rows: 1,
            num_ports_per_node: 2,
            ports_per_node_limit: None,
        },
        FabricPortConfig {
            ticks_per_hop: 2,
            ticks_overhead: overhead,
            rx_buffer_bytes: 512,
            tx_buffer_bytes: 512,
            port_bits_per_tick: 1,
        },
        HashMap::from([(1, vec![destination])]),
    )
    .unwrap()
    .with_paths(FabricPathConfig {
        bw_bits_per_tick: rate,
        queue_bytes: 512,
        queue_packets: 8,
        ..Default::default()
    })
    .unwrap();
    let fabric = RoutedFabric::<MemoryAccess>::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "mesh",
        Rc::new(config),
        FabricRoutingAlgorithm::ColumnFirst,
    )
    .unwrap();
    let mut source = OutPort::new(engine.top(), "source");
    source.connect(fabric.port_ingress_i(0)).unwrap();
    let mut sink = InPort::new(&engine, &clock, engine.top(), "sink");
    fabric
        .connect_port_egress_i(destination, sink.state())
        .unwrap();
    for port in 0..columns * 2 {
        if port != 0 {
            let mut tx = OutPort::<MemoryAccess>::new(engine.top(), &format!("unused_tx_{port}"));
            tx.connect(fabric.port_ingress_i(port)).unwrap();
        }
        if port != destination {
            let rx = InPort::<MemoryAccess>::new(
                &engine,
                &clock,
                engine.top(),
                &format!("unused_rx_{port}"),
            );
            fabric.connect_port_egress_i(port, rx.state()).unwrap();
        }
    }
    let packets: Vec<_> = sizes
        .iter()
        .enumerate()
        .map(|(i, &size)| {
            MemoryAccess::new(
                engine.top(),
                AccessType::WriteRequest,
                size,
                i as u64,
                0,
                DeviceId(1),
                DeviceId(0),
                0,
            )
        })
        .collect();
    let acks = Rc::new(RefCell::new(Vec::new()));
    let a = acks.clone();
    let c = clock.clone();
    engine.spawn(async move {
        for packet in packets {
            source.put(packet)?.await;
            a.borrow_mut().push(c.tick_now().tick());
        }
        Ok(())
    });
    let arrivals = Rc::new(RefCell::new(Vec::new()));
    let a = arrivals.clone();
    let c = clock.clone();
    let expected = sizes.to_vec();
    engine.spawn(async move {
        c.wait_ticks(stalled_until)?.await;
        for (i, size) in expected.into_iter().enumerate() {
            let packet = sink.get()?.await;
            assert_eq!(packet.dst_addr(), i as u64);
            assert_eq!(packet.total_bytes(), size);
            a.borrow_mut().push(c.tick_now().tick());
        }
        Ok(())
    });
    let during = Rc::new(RefCell::new(Vec::new()));
    let d = during.clone();
    let f = fabric.clone();
    let c = clock.clone();
    engine.spawn(async move {
        c.wait_ticks(3)?.await;
        *d.borrow_mut() = f.route_snapshots();
        Ok(())
    });
    engine.run().unwrap();
    let final_stats = fabric.route_snapshots();
    for s in &final_stats {
        assert_eq!(s.owned_bytes, 0);
        assert_eq!(s.owned_packets, 0);
        assert_eq!(s.admitted_bytes, s.completed_bytes);
        assert_eq!(s.admitted_packets, s.completed_packets);
        assert!(s.peak_bytes <= 512);
        assert!(s.peak_packets <= 8);
    }

    Observed {
        arrivals: arrivals.borrow().clone(),
        acks: acks.borrow().clone(),
        during: during.borrow().clone(),
        final_stats,
    }
}

#[test]
fn latency_is_hop_propagation_plus_one_packet_serialization() {
    for columns in [1, 2, 4] {
        for size in [0usize, 1, 64, 257, 512] {
            let observed = run(columns, 256, 3, &[size], 0);
            let expected = ((columns - 1) * 2 + 3 + (size * 8).div_ceil(256)) as u64;
            assert_eq!(
                observed.arrivals,
                vec![expected],
                "columns={columns},bytes={size}"
            );
        }
    }
}

#[test]
fn head_advances_before_tail_but_link_credit_survives_until_tail() {
    let observed = run(2, 256, 0, &[64], 0);
    let edge = observed
        .during
        .iter()
        .find(|s| s.route.ends_with("::0_0_to_1_0"))
        .unwrap();
    let delivery = observed
        .during
        .iter()
        .find(|s| s.route.ends_with("::local_3"))
        .unwrap();
    assert_eq!(
        delivery.admitted_packets, 1,
        "head has reached the delivery route by tick 3"
    );
    assert_eq!(
        edge.owned_packets, 1,
        "upstream still owns transmission credit"
    );
    assert_eq!(edge.completed_packets, 0, "tail has not completed");
    assert_eq!(observed.arrivals, vec![4]);
}

#[test]
fn fractional_link_debt_keeps_full_stream_bandwidth_through_multiple_hops() {
    let observed = run(4, 435, 3, &vec![64; 435], 0);
    assert_eq!(*observed.arrivals.last().unwrap(), 512 + 6 + 3);
    assert!(
        observed
            .final_stats
            .iter()
            .filter(|s| s.route.contains("::plane_") && s.admitted_packets > 0)
            .all(|s| s.service_ticks == 512)
    );
}

#[test]
fn blocked_destination_bounds_mixed_packets_and_preserves_fifo() {
    let sizes: Vec<_> = (0..160).map(|i| [0, 64, 16, 128][i % 4]).collect();
    let observed = run(4, 256, 3, &sizes, 1000);
    assert_eq!(observed.arrivals.len(), sizes.len());
    assert!(observed.arrivals.iter().all(|t| *t >= 1000));
    assert!(*observed.acks.last().unwrap() >= 1000);
}
