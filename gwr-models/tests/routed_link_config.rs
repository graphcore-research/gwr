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
fn positive_directional_policy_and_none() {
    assert_eq!(config(2).internal_link_bits_per_tick(), None);
    assert_eq!(
        config(2)
            .with_internal_link_bits_per_tick(Some(7))
            .unwrap()
            .internal_link_bits_per_tick(),
        Some(7)
    );
    assert_eq!(
        config(2)
            .with_internal_link_bits_per_tick(None)
            .unwrap()
            .internal_link_bits_per_tick(),
        None
    );
}

#[test]
fn zero_is_a_config_error() {
    assert!(
        config(2)
            .with_internal_link_bits_per_tick(Some(0))
            .unwrap_err()
            .to_string()
            .contains("rate must be positive")
    );
}

#[test]
fn functional_direct_constructor_rejects_opt_in() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let config = Rc::new(config(2).with_internal_link_bits_per_tick(Some(8)).unwrap());
    let error =
        match FunctionalFabric::<i32>::new_and_register(&engine, &clock, engine.top(), "f", config)
        {
            Err(e) => e,
            Ok(_) => panic!("functional accepted routed-only rate limit"),
        };
    assert!(error.to_string().contains("requires routed fabric"));
}
