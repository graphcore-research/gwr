// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::fabric::functional::FunctionalFabric;
use gwr_models::fabric::{FabricConfig, FabricGeometry, FabricPortConfig};

fn config(columns: usize) -> FabricConfig {
    FabricConfig::new(
        FabricGeometry {
            num_columns: columns,
            num_rows: 1,
            num_ports_per_node: 2,
            ports_per_node_limit: None,
        },
        FabricPortConfig {
            ticks_per_hop: 1,
            ticks_overhead: 0,
            rx_buffer_bytes: 64,
            tx_buffer_bytes: 64,
            port_bits_per_tick: 64,
        },
        HashMap::new(),
    )
    .unwrap()
}

#[test]
fn functional_direct_constructor_rejects_opt_in() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let config = Rc::new(
        config(2)
            .with_paths(gwr_models::fabric::paths::FabricPathConfig::default())
            .unwrap(),
    );
    let error =
        match FunctionalFabric::<i32>::new_and_register(&engine, &clock, engine.top(), "f", config)
        {
            Err(e) => e,
            Ok(_) => panic!("functional accepted routed-only rate limit"),
        };
    assert!(error.to_string().contains("requires routed fabric"));
}
