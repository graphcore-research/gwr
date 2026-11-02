// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::test_helpers::start_test;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::Routable;
use gwr_engine::types::{AccessType, DeviceId};
use gwr_models::fabric::functional::FunctionalFabric;
use gwr_models::fabric::{Fabric, FabricConfig, FabricGeometry, FabricPortConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::memory::striper::{MemoryStriper, MemoryStriperConfig};
use gwr_models::memory::traits::AccessMemory;
use gwr_models::memory::{Memory, MemoryConfig};

/// Catch lost parent identities or incomplete fragment accounting when a
/// striper forwards into another striper and real memories reply over a fabric.
#[test]
fn nested_stripers_complete_reads_and_writes_through_real_memories() {
    for task_order_seed in [None, Some(0), Some(1), Some(17), Some(42)] {
        run_nested_stripers_case(task_order_seed);
    }
}

fn run_nested_stripers_case(task_order_seed: Option<u64>) {
    let mut engine = start_test(file!());
    engine.set_randomize_task_order(task_order_seed.is_some());
    if let Some(seed) = task_order_seed {
        engine.set_task_order_seed(seed);
    }
    let clock = engine.default_clock();
    let outer_id = DeviceId(10);
    let (outer, banks) = build_nested_stripers(&engine, &clock);

    let mut requests = OutPort::new(engine.top(), "requests");
    requests.connect(outer.port_dev_rx()).unwrap();
    let mut responses = InPort::new(&engine, &clock, engine.top(), "responses");
    outer.connect_port_dev_tx(responses.state()).unwrap();
    let packets = [
        (AccessType::ReadRequest, 0x10013, 200, 71),
        (AccessType::WriteNonPostedRequest, 0x10013, 200, 72),
        (AccessType::WriteRequest, 0x1001f, 5, 73),
    ]
    .map(|(kind, address, bytes, slot)| {
        MemoryAccess::new(
            engine.top(),
            kind,
            bytes,
            address,
            slot,
            outer_id,
            DeviceId(99),
            8,
        )
    });
    engine.spawn(async move {
        for packet in packets {
            requests.put(packet)?.await;
        }
        Ok(())
    });
    let received = Rc::new(RefCell::new(Vec::new()));
    let observed = received.clone();
    engine.spawn(async move {
        for _ in 0..2 {
            let response = responses.get()?.await;
            observed.borrow_mut().push(response);
        }
        clock.wait_ticks(50)?.await;
        assert!(
            !responses.has_value(),
            "posted write produced an unexpected response"
        );
        Ok(())
    });
    engine.run().unwrap();

    let received = received.borrow();
    assert_eq!(received.len(), 2, "both original requests must complete");
    for (kind, slot) in [
        (AccessType::ReadResponse, 71),
        (AccessType::WriteNonPostedResponse, 72),
    ] {
        let response = received
            .iter()
            .find(|response| response.access_type() == kind)
            .unwrap();
        assert_eq!(
            (
                response.dst_addr(),
                response.src_addr(),
                response.access_size_bytes()
            ),
            (0x10013, slot, 200)
        );
        assert_eq!(
            (response.dst_device(), response.src_device()),
            (DeviceId(99), outer_id)
        );
    }
    // The 200-byte range covers 104 bytes in A and 96 in B. The posted
    // five-byte write begins on the last byte of A's first 32-byte stripe.
    assert_eq!((banks[0].bytes_read(), banks[1].bytes_read()), (104, 96));
    assert_eq!(
        (banks[0].bytes_written(), banks[1].bytes_written()),
        (105, 100)
    );
}

fn build_nested_stripers(
    engine: &Engine,
    clock: &Clock,
) -> (
    Rc<MemoryStriper<MemoryAccess>>,
    Vec<Rc<Memory<MemoryAccess>>>,
) {
    let outer_id = DeviceId(10);
    let inner_id = DeviceId(11);
    let bank_a_id = DeviceId(12);
    let bank_b_id = DeviceId(13);

    let mut outer_map = MemoryMap::new();
    outer_map.insert(0x1000, 256, inner_id).unwrap();
    let outer = MemoryStriper::new_and_register(
        engine,
        clock,
        engine.top(),
        "outer",
        MemoryStriperConfig::new(0x10000, 256, 64, outer_id, outer_map).with_admission_limits(2, 2),
    )
    .unwrap();
    let mut inner_map = MemoryMap::new();
    inner_map.insert(0x2000, 128, bank_a_id).unwrap();
    inner_map.insert(0x3000, 128, bank_b_id).unwrap();
    let inner = MemoryStriper::new_and_register(
        engine,
        clock,
        engine.top(),
        "inner",
        MemoryStriperConfig::new(0x1000, 256, 32, inner_id, inner_map).with_admission_limits(2, 2),
    )
    .unwrap();
    outer.connect_port_mem_tx(inner.port_dev_rx()).unwrap();
    inner.connect_port_dev_tx(outer.port_mem_rx()).unwrap();

    let fabric_config = FabricConfig::new(
        FabricGeometry {
            num_columns: 3,
            num_rows: 1,
            num_ports_per_node: 1,
            ports_per_node_limit: None,
        },
        FabricPortConfig {
            ticks_per_hop: 1,
            ticks_overhead: 1,
            rx_buffer_bytes: 1024,
            tx_buffer_bytes: 1024,
            port_bits_per_tick: 256,
        },
        HashMap::from([
            (inner_id.0, vec![0]),
            (bank_a_id.0, vec![1]),
            (bank_b_id.0, vec![2]),
        ]),
    )
    .unwrap();
    let fabric = FunctionalFabric::new_and_register(
        engine,
        clock,
        engine.top(),
        "backing_fabric",
        Rc::new(fabric_config),
    )
    .unwrap();
    inner.connect_port_mem_tx(fabric.port_ingress_i(0)).unwrap();
    fabric
        .connect_port_egress_i(0, inner.port_mem_rx())
        .unwrap();
    let mut banks = Vec::new();
    for (index, base, delay) in [(1, 0x2000, 7), (2, 0x3000, 1)] {
        let bank = Memory::<MemoryAccess>::new_and_register(
            engine,
            clock,
            engine.top(),
            &format!("bank_{index}"),
            MemoryConfig::new(base, 128, 16, delay),
        )
        .unwrap();
        bank.connect_port_tx(fabric.port_ingress_i(index)).unwrap();
        fabric.connect_port_egress_i(index, bank.port_rx()).unwrap();
        banks.push(bank);
    }

    (outer, banks)
}
