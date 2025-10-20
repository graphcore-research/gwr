// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::port::{InPort, OutPort};
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::{Routable, SimObject};
use gwr_models::fabric::FabricConfig;
use gwr_models::fabric::node::{FabricNode, FabricRoutingAlgoritm};
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

fn connect_ingress_egress<T>(top: &Rc<Entity>, node: &Rc<FabricNode<T>>, config: &Rc<FabricConfig>)
where
    T: SimObject + Routable,
{
    for i in 0..config.num_ports_per_node() {
        let mut port = OutPort::new(top, "port");
        port.connect(node.port_ingress_i(i)).unwrap();
        let port = InPort::new(top, "port");
        node.connect_port_egress_i(i, port.state()).unwrap();
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
        top,
        "node",
        0,
        0,
        clock,
        &config,
        FabricRoutingAlgoritm::ColumnFirst,
    )
    .unwrap();

    let mut port = OutPort::new(top, "port");
    port.connect(node.port_col_minus()).unwrap();
    let mut port = OutPort::new(top, "port");
    port.connect(node.port_col_plus()).unwrap();
    let mut port = OutPort::new(top, "port");
    port.connect(node.port_row_minus()).unwrap();
    let mut port = OutPort::new(top, "port");
    port.connect(node.port_row_plus()).unwrap();

    let port = InPort::new(top, "port");
    node.connect_port_col_minus(port.state()).unwrap();
    let port = InPort::new(top, "port");
    node.connect_port_col_plus(port.state()).unwrap();
    let port = InPort::new(top, "port");
    node.connect_port_row_minus(port.state()).unwrap();
    let port = InPort::new(top, "port");
    node.connect_port_row_plus(port.state()).unwrap();

    connect_ingress_egress(top, &node, &config);
}
