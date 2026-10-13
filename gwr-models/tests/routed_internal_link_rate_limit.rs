// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::TotalBytes;
use gwr_models::ethernet_frame::{EthernetFrame, FRAME_OVERHEAD_BYTES, u64_to_mac};
use gwr_models::fabric::node::FabricRoutingAlgorithm;
use gwr_models::fabric::routed::RoutedFabric;
use gwr_models::fabric::{Fabric, FabricConfig, FabricGeometry, FabricPortConfig};

const PAYLOAD_BYTES: usize = 40;

#[derive(Clone)]
struct Stream {
    id: u64,
    src: FabricEndpoint,
    dst: FabricEndpoint,
    payloads: Vec<usize>,
    receive_after: u64,
}

#[derive(Clone)]
struct FabricEndpoint {
    column: usize,
    row: usize,
    port: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Arrival {
    token: u64,
    dst: u64,
    bytes: usize,
    tick: u64,
}

#[derive(Debug, PartialEq)]
struct Observed {
    arrivals: Vec<Arrival>,
    producer_acks: Vec<(u64, u64)>,
    whole_ticks: u64,
    whole_ns: f64,
}

struct TestPorts {
    sources: Vec<Option<OutPort<EthernetFrame>>>,
    sinks: Vec<Option<InPort<EthernetFrame>>>,
}

#[test]
fn shared_edge_distinct_endpoints_requires_rate_limiting() {
    // Both streams use only (0,0)->(1,0), while ingress and egress ports
    // differ.
    for algorithm in [
        FabricRoutingAlgorithm::ColumnFirst,
        FabricRoutingAlgorithm::RowFirst,
    ] {
        let streams = [
            stream(
                1,
                endpoint(0, 0, 0),
                endpoint(1, 0, 0),
                vec![PAYLOAD_BYTES; 4],
            ),
            stream(
                2,
                endpoint(0, 0, 1),
                endpoint(1, 0, 1),
                vec![PAYLOAD_BYTES; 4],
            ),
        ];
        let baseline = run_and_observe(2, 1, None, 2, algorithm, 1000.0, &streams);
        assert_eq!(
            baseline,
            run_and_observe(2, 1, None, 2, algorithm, 1000.0, &streams),
            "complete repeated None parity"
        );
        let slow = run_and_observe(2, 1, Some(8), 2, algorithm, 1000.0, &streams);
        let fast = run_and_observe(2, 1, Some(16), 2, algorithm, 1000.0, &streams);
        let arrivals = sorted_arrivals(&slow);
        let duration = ((PAYLOAD_BYTES + FRAME_OVERHEAD_BYTES) * 8).div_ceil(8) as u64;
        assert!(
            arrivals
                .windows(2)
                .all(|p| p[1].tick - p[0].tick >= duration),
            "actual shared-edge serialized spacing, not endpoint contention"
        );
        assert_eq!(
            arrivals[0].tick,
            sorted_arrivals(&baseline)[0].tick,
            "forward-first law retains isolated first arrival"
        );
        assert!(slow.whole_ticks > fast.whole_ticks && fast.whole_ticks > baseline.whole_ticks);
    }
}

#[test]
fn reverse_directions_have_independent_rate_limits() {
    for algorithm in [
        FabricRoutingAlgorithm::ColumnFirst,
        FabricRoutingAlgorithm::RowFirst,
    ] {
        let forward = stream(
            1,
            endpoint(0, 0, 0),
            endpoint(1, 0, 0),
            vec![PAYLOAD_BYTES; 4],
        );
        let reverse = stream(
            2,
            endpoint(1, 0, 0),
            endpoint(0, 0, 0),
            vec![PAYLOAD_BYTES; 4],
        );
        let a = run_and_observe(
            2,
            1,
            Some(8),
            2,
            algorithm,
            1000.0,
            std::slice::from_ref(&forward),
        );
        let b = run_and_observe(
            2,
            1,
            Some(8),
            2,
            algorithm,
            1000.0,
            std::slice::from_ref(&reverse),
        );
        let pair = run_and_observe(2, 1, Some(8), 2, algorithm, 1000.0, &[forward, reverse]);
        assert_eq!(arrival_ticks(&pair, 1), arrival_ticks(&a, 1));
        assert_eq!(arrival_ticks(&pair, 2), arrival_ticks(&b, 2));
        assert_eq!(pair.whole_ticks, a.whole_ticks.max(b.whole_ticks));
    }
}

#[test]
fn disjoint_edges_and_vertical_links_are_independent() {
    for algorithm in [
        FabricRoutingAlgorithm::ColumnFirst,
        FabricRoutingAlgorithm::RowFirst,
    ] {
        for vertical in [false, true] {
            let (a, b) = if vertical {
                (
                    stream(
                        1,
                        endpoint(0, 0, 0),
                        endpoint(0, 1, 0),
                        vec![PAYLOAD_BYTES; 4],
                    ),
                    stream(
                        2,
                        endpoint(1, 0, 0),
                        endpoint(1, 1, 0),
                        vec![PAYLOAD_BYTES; 4],
                    ),
                )
            } else {
                (
                    stream(
                        1,
                        endpoint(0, 0, 0),
                        endpoint(1, 0, 0),
                        vec![PAYLOAD_BYTES; 4],
                    ),
                    stream(
                        2,
                        endpoint(0, 1, 0),
                        endpoint(1, 1, 0),
                        vec![PAYLOAD_BYTES; 4],
                    ),
                )
            };
            let alone_a = run_and_observe(
                2,
                2,
                Some(8),
                2,
                algorithm,
                1000.0,
                std::slice::from_ref(&a),
            );
            let alone_b = run_and_observe(
                2,
                2,
                Some(8),
                2,
                algorithm,
                1000.0,
                std::slice::from_ref(&b),
            );
            let pair = run_and_observe(2, 2, Some(8), 2, algorithm, 1000.0, &[a, b]);
            assert_eq!(arrival_ticks(&pair, 1), arrival_ticks(&alone_a, 1));
            assert_eq!(arrival_ticks(&pair, 2), arrival_ticks(&alone_b, 2));
            assert_eq!(
                pair.whole_ticks,
                alone_a.whole_ticks.max(alone_b.whole_ticks)
            );
        }
    }
}

#[test]
fn irregular_packet_headers_and_non_one_ns_clock_keep_tick_law() {
    let spec = stream(1, endpoint(0, 0, 0), endpoint(1, 0, 0), vec![0, 1, 43, 7]);
    let rate = 7;
    let a = run_and_observe(
        2,
        1,
        Some(rate),
        2,
        FabricRoutingAlgorithm::ColumnFirst,
        1000.0,
        std::slice::from_ref(&spec),
    );
    let b = run_and_observe(
        2,
        1,
        Some(rate),
        2,
        FabricRoutingAlgorithm::ColumnFirst,
        500.0,
        &[spec],
    );
    assert_eq!(
        a.arrivals.iter().map(|a| a.tick).collect::<Vec<_>>(),
        b.arrivals.iter().map(|a| a.tick).collect::<Vec<_>>()
    );
    assert_eq!(a.producer_acks, b.producer_acks);
    assert_eq!(a.whole_ticks, b.whole_ticks);
    assert_eq!(b.whole_ns, 2.0 * a.whole_ns);
    let ordered = sorted_arrivals(&a);
    for pair in ordered.windows(2) {
        let expected = (pair[0].bytes * 8).div_ceil(rate) as u64;
        assert!(pair[1].tick - pair[0].tick >= expected);
    }
    assert_eq!(
        a.arrivals[0].bytes, FRAME_OVERHEAD_BYTES,
        "zero data payload still charges existing declared frame overhead"
    );
}

#[test]
fn downstream_stall_backpressures_producers_with_and_without_link_rate_limit() {
    const RECEIVE_AFTER: u64 = 10000;

    for algorithm in [
        FabricRoutingAlgorithm::ColumnFirst,
        FabricRoutingAlgorithm::RowFirst,
    ] {
        for hop_ticks in [1, 2, 4] {
            for rate in [None, Some(8)] {
                let mut accepted_before_stall = Vec::new();
                for packets in [32, 64] {
                    let mut spec = stream(
                        1,
                        endpoint(0, 0, 0),
                        endpoint(1, 1, 0),
                        vec![PAYLOAD_BYTES; packets],
                    );
                    spec.receive_after = RECEIVE_AFTER;
                    let observed =
                        run_and_observe(2, 2, rate, hop_ticks, algorithm, 1000.0, &[spec]);
                    let early_acks = observed
                        .producer_acks
                        .iter()
                        .filter(|&&(_, tick)| tick < RECEIVE_AFTER)
                        .count();
                    assert!(
                        early_acks < packets,
                        "blocked egress must block the producer"
                    );
                    assert!(observed.producer_acks.last().unwrap().1 >= RECEIVE_AFTER);
                    assert!(
                        observed
                            .arrivals
                            .iter()
                            .all(|arrival| arrival.tick >= RECEIVE_AFTER)
                    );
                    accepted_before_stall.push(early_acks);
                }
                assert_eq!(
                    accepted_before_stall[0], accepted_before_stall[1],
                    "in-flight capacity must not grow with the offered traffic",
                );
            }
        }
    }
}

#[test]
fn multi_hop_routes_and_receiver_stall_preserve_order_and_packets() {
    for algorithm in [
        FabricRoutingAlgorithm::ColumnFirst,
        FabricRoutingAlgorithm::RowFirst,
    ] {
        let immediate = stream(
            1,
            endpoint(0, 0, 0),
            endpoint(1, 1, 0),
            vec![PAYLOAD_BYTES; 12],
        );
        let mut stalled = immediate.clone();
        stalled.receive_after = 5000;
        let none = run_and_observe(
            2,
            2,
            None,
            2,
            algorithm,
            1000.0,
            std::slice::from_ref(&immediate),
        );
        assert_eq!(
            none,
            run_and_observe(
                2,
                2,
                None,
                2,
                algorithm,
                1000.0,
                std::slice::from_ref(&immediate)
            )
        );
        let fast = run_and_observe(2, 2, Some(8), 2, algorithm, 1000.0, &[immediate]);
        let slow = run_and_observe(2, 2, Some(8), 2, algorithm, 1000.0, &[stalled]);
        assert!(fast.whole_ticks > none.whole_ticks);
        assert!(slow.whole_ticks > fast.whole_ticks);
        assert!(slow.arrivals.iter().all(|a| a.tick >= 5000));
        assert_eq!(
            fast.arrivals
                .iter()
                .map(|a| (a.token, a.dst, a.bytes))
                .collect::<Vec<_>>(),
            slow.arrivals
                .iter()
                .map(|a| (a.token, a.dst, a.bytes))
                .collect::<Vec<_>>()
        );
    }
}

fn stream(id: u64, src: FabricEndpoint, dst: FabricEndpoint, payloads: Vec<usize>) -> Stream {
    Stream {
        id,
        src,
        dst,
        payloads,
        receive_after: 0,
    }
}

fn endpoint(column: usize, row: usize, port: usize) -> FabricEndpoint {
    FabricEndpoint { column, row, port }
}

fn run_and_observe(
    cols: usize,
    rows: usize,
    rate: Option<usize>,
    hop_ticks: usize,
    algorithm: FabricRoutingAlgorithm,
    mhz: f64,
    streams: &[Stream],
) -> Observed {
    let mut engine = Engine::default();
    let clock = engine.clock_mhz(mhz);
    let config = build_routed_config(cols, rows, rate, hop_ticks);
    let count = config.num_ports();
    let fabric = RoutedFabric::<EthernetFrame>::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "fabric",
        config.clone(),
        algorithm,
    )
    .unwrap();
    let mut ports = connect_test_ports(&engine, &clock, &fabric, count);
    let arrivals = Rc::new(RefCell::new(Vec::new()));
    let acks = Rc::new(RefCell::new(Vec::new()));
    let mut expected = Vec::new();
    for spec in streams {
        let src =
            config.col_row_port_to_fabric_port_index(spec.src.column, spec.src.row, spec.src.port);
        let dst =
            config.col_row_port_to_fabric_port_index(spec.dst.column, spec.dst.row, spec.dst.port);
        let source = ports.sources[src]
            .take()
            .expect("distinct stream ingress required");
        let sink = ports.sinks[dst]
            .take()
            .expect("distinct stream egress required");
        let frames = build_stream_frames(engine.top(), spec, dst, &mut expected);
        spawn_receiver(
            &engine,
            &clock,
            sink,
            frames.len(),
            spec.receive_after,
            arrivals.clone(),
        );
        spawn_producer(&engine, &clock, source, frames, acks.clone());
    }
    engine.run().unwrap();
    let mut observed = Observed {
        arrivals: arrivals.borrow().clone(),
        producer_acks: acks.borrow().clone(),
        whole_ticks: clock.tick_now().tick(),
        whole_ns: engine.time_now_ns(),
    };
    validate_observed(&mut observed, streams, expected);
    println!("routed_native cols={cols} rows={rows} rate={rate:?} mhz={mhz} observed={observed:?}");
    observed
}

fn build_routed_config(
    cols: usize,
    rows: usize,
    rate: Option<usize>,
    hop_ticks: usize,
) -> Rc<FabricConfig> {
    let count = cols * rows * 2;
    Rc::new(
        FabricConfig::new(
            FabricGeometry {
                num_columns: cols,
                num_rows: rows,
                num_ports_per_node: 2,
                ports_per_node_limit: None,
            },
            FabricPortConfig {
                ticks_per_hop: hop_ticks,
                ticks_overhead: 0,
                rx_buffer_bytes: 256,
                tx_buffer_bytes: 256,
                port_bits_per_tick: 65536,
            },
            (0..count)
                .map(|i| (i as u64, vec![i]))
                .collect::<HashMap<_, _>>(),
        )
        .unwrap()
        .with_internal_link_bits_per_tick(rate)
        .unwrap(),
    )
}

fn connect_test_ports(
    engine: &Engine,
    clock: &Clock,
    fabric: &RoutedFabric<EthernetFrame>,
    count: usize,
) -> TestPorts {
    let mut sources = Vec::new();
    let mut sinks = Vec::new();
    for i in 0..count {
        let mut src = OutPort::new(engine.top(), &format!("source_{i}"));
        src.connect(fabric.port_ingress_i(i)).unwrap();
        sources.push(Some(src));
        let sink = InPort::new(engine, clock, engine.top(), &format!("sink_{i}"));
        fabric.connect_port_egress_i(i, sink.state()).unwrap();
        sinks.push(Some(sink));
    }
    TestPorts { sources, sinks }
}

fn spawn_receiver(
    engine: &Engine,
    clock: &Clock,
    mut sink: InPort<EthernetFrame>,
    packets: usize,
    receive_after: u64,
    arrivals: Rc<RefCell<Vec<Arrival>>>,
) {
    let clock = clock.clone();
    engine.spawn(async move {
        clock.wait_ticks(receive_after)?.await;
        for _ in 0..packets {
            let frame = sink.get()?.await;
            arrivals.borrow_mut().push(Arrival {
                token: frame.get_src(),
                dst: frame.get_dst(),
                bytes: frame.total_bytes(),
                tick: clock.tick_now().tick(),
            });
        }
        Ok(())
    });
}

fn spawn_producer(
    engine: &Engine,
    clock: &Clock,
    mut source: OutPort<EthernetFrame>,
    frames: Vec<EthernetFrame>,
    acks: Rc<RefCell<Vec<(u64, u64)>>>,
) {
    let clock = clock.clone();
    engine.spawn(async move {
        for frame in frames {
            let token = frame.get_src();
            source.put(frame)?.await;
            acks.borrow_mut().push((token, clock.tick_now().tick()));
        }
        Ok(())
    });
}

fn build_stream_frames(
    parent: &Rc<gwr_track::entity::Entity>,
    spec: &Stream,
    dst: usize,
    expected: &mut Vec<(u64, u64, usize)>,
) -> Vec<EthernetFrame> {
    spec.payloads
        .iter()
        .enumerate()
        .map(|(seq, &bytes)| {
            assert!(
                bytes + FRAME_OVERHEAD_BYTES <= 256,
                "explicit buffers must admit each packet"
            );
            let token = (spec.id << 32) | seq as u64;
            expected.push((token, dst as u64, bytes + FRAME_OVERHEAD_BYTES));
            EthernetFrame::new(parent, bytes)
                .set_src(u64_to_mac(token))
                .set_dest(u64_to_mac(dst as u64))
        })
        .collect()
}

fn validate_observed(
    observed: &mut Observed,
    streams: &[Stream],
    mut expected: Vec<(u64, u64, usize)>,
) {
    observed.arrivals.sort_by_key(|a| a.token);
    observed.producer_acks.sort_by_key(|a| a.0);
    expected.sort_unstable();
    assert_eq!(
        observed
            .arrivals
            .iter()
            .map(|a| (a.token, a.dst, a.bytes))
            .collect::<Vec<_>>(),
        expected,
        "complete route/packet/header conservation"
    );
    assert_eq!(
        observed
            .producer_acks
            .iter()
            .map(|a| a.0)
            .collect::<Vec<_>>(),
        expected.iter().map(|a| a.0).collect::<Vec<_>>(),
        "every retained real producer future completed"
    );
    for spec in streams {
        let ordered: Vec<_> = observed
            .arrivals
            .iter()
            .filter(|a| a.token >> 32 == spec.id)
            .collect();
        assert!(ordered.windows(2).all(|p| p[1].tick >= p[0].tick));
    }
}

fn arrival_ticks(observed: &Observed, id: u64) -> Vec<u64> {
    observed
        .arrivals
        .iter()
        .filter(|a| a.token >> 32 == id)
        .map(|a| a.tick)
        .collect()
}

fn sorted_arrivals(observed: &Observed) -> Vec<&Arrival> {
    let mut a: Vec<_> = observed.arrivals.iter().collect();
    a.sort_by_key(|a| a.tick);
    a
}
