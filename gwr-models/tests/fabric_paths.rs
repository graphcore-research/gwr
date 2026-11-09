// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::fabric::paths::{
    FabricAddressKey, FabricPathConfig, FabricPathSelection, FabricPathSelector,
};

#[test]
fn xor_fold_balances_local_lines_and_ignores_byte_offsets() {
    let hash = FabricPathSelection::AddressXorFold { address_shift: 6 };
    let expected = [0, 1, 2, 3, 1, 0, 3, 2];
    for (line, want) in expected.into_iter().enumerate() {
        for byte in [0, 1, 63] {
            assert_eq!(
                hash.select(
                    FabricAddressKey {
                        memory_index: 0,
                        offset_in_memory: line as u64 * 64 + byte
                    },
                    4
                )
                .unwrap(),
                want
            );
        }
    }
    for memory in 0..128 {
        let mut counts = [0; 4];
        for line in 0..128 {
            counts[hash
                .select(
                    FabricAddressKey {
                        memory_index: memory,
                        offset_in_memory: line * 64,
                    },
                    4,
                )
                .unwrap()] += 1;
        }
        assert_eq!(counts, [32; 4]);
    }
}

#[test]
fn selection_validates_counts_shifts_and_boundary_values() {
    let key = FabricAddressKey {
        memory_index: u64::MAX,
        offset_in_memory: u64::MAX,
    };
    let xor = FabricPathSelection::AddressXorFold { address_shift: 63 };
    assert_eq!(xor.select(key, 1).unwrap(), 0);
    assert!(xor.select(key, 3).is_err());
    assert!(xor.select(key, 0).is_err());
    assert!(
        FabricPathSelection::AddressXorFold { address_shift: 64 }
            .select(key, 4)
            .is_err()
    );
    let modulo = FabricPathSelection::AddressMixModulo { address_shift: 0 };
    assert!(modulo.select(key, 3).unwrap() < 3);
    let mut config = FabricPathConfig {
        count: 0,
        ..FabricPathConfig::default()
    };
    assert!(config.validate().is_err());
    config.count = 4;
    config.bw_bits_per_tick = 0;
    assert!(config.validate().is_err());
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep complete setup execution and assertions of this scenario together"
)]
fn routed_stream(
    paths: usize,
    same_address: bool,
) -> (u64, Vec<gwr_models::fabric::service::RouteServiceSnapshot>) {
    use std::collections::HashMap;
    use std::rc::Rc;

    use gwr_engine::engine::Engine;
    use gwr_engine::port::{InPort, OutPort};
    use gwr_engine::traits::Routable;
    use gwr_engine::types::{AccessType, DeviceId};
    use gwr_models::fabric::node::FabricRoutingAlgorithm;
    use gwr_models::fabric::routed::RoutedFabric;
    use gwr_models::fabric::{Fabric, FabricConfig, FabricGeometry, FabricPortConfig};
    use gwr_models::memory::memory_access::MemoryAccess;
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let config = Rc::new(
        FabricConfig::new(
            FabricGeometry {
                num_columns: 2,
                num_rows: 1,
                num_ports_per_node: 2,
                ports_per_node_limit: None,
            },
            FabricPortConfig {
                ticks_per_hop: 2,
                ticks_overhead: 0,
                rx_buffer_bytes: 8192,
                tx_buffer_bytes: 8192,
                port_bits_per_tick: 1,
            },
            HashMap::from([(1, vec![2])]),
        )
        .unwrap(),
    );
    let path_config = FabricPathConfig {
        count: paths,
        bw_bits_per_tick: 435,
        ..Default::default()
    };
    let selector = Rc::new(path_config.selection);
    let fabric = RoutedFabric::new_and_register_with_paths(
        &engine,
        &clock,
        engine.top(),
        "mesh",
        config,
        FabricRoutingAlgorithm::ColumnFirst,
        path_config,
        Rc::new(|p: &MemoryAccess| {
            Ok(FabricAddressKey {
                memory_index: 0,
                offset_in_memory: p.dst_addr(),
            })
        }),
        selector,
    )
    .unwrap();
    let mut source = OutPort::new(engine.top(), "source");
    source.connect(fabric.port_ingress_i(0)).unwrap();
    let mut sink = InPort::new(&engine, &clock, engine.top(), "sink");
    fabric.connect_port_egress_i(2, sink.state()).unwrap();
    for i in 0..4 {
        if i != 0 {
            let mut unused =
                OutPort::<MemoryAccess>::new(engine.top(), &format!("unused_source_{i}"));
            unused.connect(fabric.port_ingress_i(i)).unwrap();
        }
        if i != 2 {
            let unused = InPort::<MemoryAccess>::new(
                &engine,
                &clock,
                engine.top(),
                &format!("unused_sink_{i}"),
            );
            fabric.connect_port_egress_i(i, unused.state()).unwrap();
        }
    }
    let packets: Vec<_> = (0..1024)
        .map(|i| {
            MemoryAccess::new(
                engine.top(),
                AccessType::WriteRequest,
                64,
                if same_address { 0 } else { i * 64 },
                0,
                DeviceId(1),
                DeviceId(0),
                0,
            )
        })
        .collect();
    engine.spawn(async move {
        for p in packets {
            source.put(p)?.await;
        }
        Ok(())
    });
    engine.spawn(async move {
        for _ in 0..1024 {
            sink.get()?.await;
        }
        Ok(())
    });
    engine.run().unwrap();
    let stats = fabric.route_snapshots();
    assert!(
        stats
            .iter()
            .all(|s| s.owned_packets == 0 && s.owned_bytes == 0)
    );
    (clock.tick_now().tick(), stats)
}
#[test]
fn independent_planes_scale_without_an_external_port_rate_cap() {
    let (one, _) = routed_stream(1, false);
    let (four, _) = routed_stream(4, false);
    assert!(four * 35 < one * 10, "one={one},four={four}");
    let (same, _) = routed_stream(4, true);
    assert!(same >= one - 4, "same={same},one={one}");
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep complete setup execution and assertions of this scenario together"
)]
fn a_blocked_plane_leaves_other_admitted_plane_traffic_progressing() {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use gwr_engine::engine::Engine;
    use gwr_engine::port::{InPort, OutPort};
    use gwr_engine::traits::Routable;
    use gwr_engine::types::{AccessType, DeviceId};
    use gwr_models::fabric::node::FabricRoutingAlgorithm;
    use gwr_models::fabric::routed::RoutedFabric;
    use gwr_models::fabric::{Fabric, FabricConfig, FabricGeometry, FabricPortConfig};
    use gwr_models::memory::memory_access::MemoryAccess;
    let mut engine = Engine::default();
    let c = engine.default_clock();
    let config = Rc::new(
        FabricConfig::new(
            FabricGeometry {
                num_columns: 2,
                num_rows: 1,
                num_ports_per_node: 2,
                ports_per_node_limit: None,
            },
            FabricPortConfig {
                ticks_per_hop: 2,
                ticks_overhead: 0,
                rx_buffer_bytes: 64,
                tx_buffer_bytes: 64,
                port_bits_per_tick: 1,
            },
            HashMap::from([(1, vec![2]), (2, vec![3])]),
        )
        .unwrap(),
    );
    let paths = FabricPathConfig {
        count: 2,
        bw_bits_per_tick: 8,
        queue_bytes: 512,
        queue_packets: 8,
        ..Default::default()
    };
    let selector = Rc::new(paths.selection);
    let f = RoutedFabric::new_and_register_with_paths(
        &engine,
        &c,
        engine.top(),
        "mesh",
        config,
        FabricRoutingAlgorithm::ColumnFirst,
        paths,
        Rc::new(|p: &MemoryAccess| {
            Ok(FabricAddressKey {
                memory_index: 0,
                offset_in_memory: p.dst_addr(),
            })
        }),
        selector,
    )
    .unwrap();
    let mut source = OutPort::new(engine.top(), "source");
    source.connect(f.port_ingress_i(0)).unwrap();
    for i in 1..4 {
        let mut unused = OutPort::<MemoryAccess>::new(engine.top(), &format!("unused_{i}"));
        unused.connect(f.port_ingress_i(i)).unwrap();
    }
    for i in 0..2 {
        let unused =
            InPort::<MemoryAccess>::new(&engine, &c, engine.top(), &format!("unused_out_{i}"));
        f.connect_port_egress_i(i, unused.state()).unwrap();
    }
    let observed = Rc::new(RefCell::new(Vec::new()));
    for destination in [2, 3] {
        let mut sink =
            InPort::<MemoryAccess>::new(&engine, &c, engine.top(), &format!("sink_{destination}"));
        f.connect_port_egress_i(destination, sink.state()).unwrap();
        let c = c.clone();
        let out = observed.clone();
        engine.spawn(async move {
            if destination == 2 {
                c.wait_ticks(10000)?.await;
            }
            for seq in 0..4 {
                let p = sink.get()?.await;
                assert_eq!(p.src_addr(), seq);
                out.borrow_mut().push((destination, c.tick_now().tick()));
            }
            Ok(())
        });
    }
    let packets: Vec<_> = [(1, 0), (2, 64)]
        .into_iter()
        .flat_map(|(memory, addr)| (0..4).map(move |seq| (memory, addr, seq)))
        .map(|(memory, addr, seq)| {
            MemoryAccess::new(
                engine.top(),
                AccessType::WriteRequest,
                64,
                addr,
                seq,
                DeviceId(memory),
                DeviceId(0),
                0,
            )
        })
        .collect();
    engine.spawn(async move {
        for p in packets {
            source.put(p)?.await;
        }
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(
        observed
            .borrow()
            .iter()
            .filter(|(dest, t)| *dest == 3 && *t < 10000)
            .count(),
        4
    );
    assert!(f.route_snapshots().iter().all(|s| s.owned_packets == 0));
}

#[test]
fn overlapping_addresses_choose_one_gateway_attachment() {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use gwr_engine::engine::Engine;
    use gwr_engine::port::{InPort, OutPort};
    use gwr_engine::traits::Routable;
    use gwr_engine::types::{AccessType, DeviceId};
    use gwr_models::fabric::node::FabricRoutingAlgorithm;
    use gwr_models::fabric::routed::RoutedFabric;
    use gwr_models::fabric::{Fabric, FabricConfig, FabricGeometry, FabricPortConfig};
    use gwr_models::memory::memory_access::MemoryAccess;
    let mut e = Engine::default();
    let c = e.default_clock();
    let cfg = Rc::new(
        FabricConfig::new(
            FabricGeometry {
                num_columns: 2,
                num_rows: 1,
                num_ports_per_node: 2,
                ports_per_node_limit: None,
            },
            FabricPortConfig {
                ticks_per_hop: 2,
                ticks_overhead: 0,
                rx_buffer_bytes: 8192,
                tx_buffer_bytes: 8192,
                port_bits_per_tick: 1,
            },
            HashMap::from([(1, vec![2, 3])]),
        )
        .unwrap(),
    );
    let paths = FabricPathConfig {
        count: 4,
        ..Default::default()
    };
    let f = RoutedFabric::new_and_register_with_paths(
        &e,
        &c,
        e.top(),
        "fabric",
        cfg,
        FabricRoutingAlgorithm::ColumnFirst,
        paths.clone(),
        Rc::new(|p: &MemoryAccess| {
            Ok(FabricAddressKey {
                memory_index: 0,
                offset_in_memory: p.dst_addr(),
            })
        }),
        Rc::new(paths.selection),
    )
    .unwrap();
    let mut source = OutPort::new(e.top(), "source");
    source.connect(f.port_ingress_i(0)).unwrap();
    for i in 1..4 {
        let mut idle = OutPort::<MemoryAccess>::new(e.top(), &format!("idle_{i}"));
        idle.connect(f.port_ingress_i(i)).unwrap();
    }
    let destinations = Rc::new(RefCell::new(Vec::new()));
    for i in 0..4 {
        let mut sink = InPort::<MemoryAccess>::new(&e, &c, e.top(), &format!("sink_{i}"));
        f.connect_port_egress_i(i, sink.state()).unwrap();
        let out = destinations.clone();
        e.spawn(async move {
            loop {
                sink.get()?.await;
                out.borrow_mut().push(i);
            }
            #[allow(unreachable_code)]
            Ok(())
        });
    }
    let packets: Vec<_> = (0..64)
        .map(|addr| {
            MemoryAccess::new(
                e.top(),
                AccessType::ReadRequest,
                1,
                addr,
                0,
                DeviceId(1),
                DeviceId(100),
                0,
            )
        })
        .collect();
    e.spawn(async move {
        for p in packets {
            source.put(p)?.await;
        }
        Ok(())
    });
    e.run().unwrap();
    let got = destinations.borrow();
    assert_eq!(got.len(), 64);
    assert!(
        got.iter().all(|d| *d == got[0]),
        "same ordering region split across attachments: {got:?}"
    );
}

#[test]
fn multiple_planes_require_an_explicit_canonical_protocol_provider() {
    use std::collections::HashMap;
    use std::rc::Rc;

    use gwr_engine::engine::Engine;
    use gwr_models::fabric::node::FabricRoutingAlgorithm;
    use gwr_models::fabric::routed::RoutedFabric;
    use gwr_models::fabric::{FabricConfig, FabricGeometry, FabricPortConfig};
    let mut e = Engine::default();
    let c = e.default_clock();
    let cfg = Rc::new(
        FabricConfig::new(
            FabricGeometry {
                num_columns: 2,
                num_rows: 1,
                num_ports_per_node: 2,
                ports_per_node_limit: None,
            },
            FabricPortConfig {
                ticks_per_hop: 2,
                ticks_overhead: 0,
                rx_buffer_bytes: 8192,
                tx_buffer_bytes: 8192,
                port_bits_per_tick: 1,
            },
            HashMap::new(),
        )
        .unwrap()
        .with_paths(FabricPathConfig {
            count: 4,
            ..Default::default()
        })
        .unwrap(),
    );
    assert!(
        RoutedFabric::<i32>::new_and_register(
            &e,
            &c,
            e.top(),
            "fabric",
            cfg,
            FabricRoutingAlgorithm::ColumnFirst
        )
        .is_err()
    );
}
