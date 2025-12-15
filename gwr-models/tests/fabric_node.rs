// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_components::{connect_dummy_rx, connect_dummy_tx};
use gwr_engine::engine::Engine;
use gwr_engine::test_helpers::start_test;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Routable, SimObject};
use gwr_models::fabric::FabricConfig;
use gwr_models::fabric::node::{FabricNode, FabricRoutingAlgorithm};
use gwr_track::entity::Entity;

fn default_config() -> Rc<FabricConfig> {
    let num_columns = 3;
    let num_rows = 4;
    let num_ports_per_node = 2;
    let cycles_per_hop = 5;
    let cycles_overhead = 1;
    let rx_buffer_entries = 1;
    let tx_buffer_entries = 1;
    let port_bits_per_tick = 128;

    let config = FabricConfig::new(
        num_columns,
        num_rows,
        num_ports_per_node,
        None,
        cycles_per_hop,
        cycles_overhead,
        rx_buffer_entries,
        tx_buffer_entries,
        port_bits_per_tick,
    );
    Rc::new(config)
}

fn connect_ingress_egress<T>(
    engine: &Engine,
    clock: &Clock,
    top: &Rc<Entity>,
    node: &Rc<FabricNode<T>>,
    config: &Rc<FabricConfig>,
) where
    T: SimObject + Routable,
{
    for i in 0..config.num_ports_per_node() {
        connect_dummy_tx!(top => node, ingress, i).unwrap();
        connect_dummy_rx!(node, egress, i => engine, clock, top).unwrap();
    }
}

#[test]
fn all_ports_connect() {
    let mut engine = start_test(file!());
    let clock = engine.clock_ghz(1.0);
    let top = engine.top();
    let config = default_config();
    let node: Rc<FabricNode<usize>> = FabricNode::new_and_register(
        &engine,
        &clock,
        top,
        "node",
        0,
        0,
        &config,
        FabricRoutingAlgorithm::ColumnFirst,
    )
    .unwrap();

    connect_dummy_tx!(top => node, col_minus).unwrap();
    connect_dummy_tx!(top => node, col_plus).unwrap();
    connect_dummy_tx!(top => node, row_minus).unwrap();
    connect_dummy_tx!(top => node, row_plus).unwrap();

    connect_dummy_rx!(node, col_minus => &engine, &clock, top).unwrap();
    connect_dummy_rx!(node, col_plus => &engine, &clock, top).unwrap();
    connect_dummy_rx!(node, row_minus => &engine, &clock, top).unwrap();
    connect_dummy_rx!(node, row_plus => &engine, &clock, top).unwrap();

    connect_ingress_egress(&engine, &clock, top, &node, &config);
}
