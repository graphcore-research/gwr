// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::time::clock::Clock;
use gwr_engine::types::SimResult;
use gwr_models::cache::coherency_manager::CoherencyManager;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_track::entity::GetEntity;

use crate::Platform;
use crate::connection_id::CachePortId;
use crate::resolved::Endpoint;

pub(crate) fn connect_ports(
    engine: &Engine,
    clock: &Clock,
    platform: &Platform,
    managers: &[Rc<CoherencyManager>],
    connections: &[[Endpoint; 2]],
) -> SimResult {
    let receive = |endpoint| match endpoint {
        Endpoint::Pe(index) => platform.processing_elements[index].port_rx(),
        Endpoint::Memory(index) => platform.memories[index].port_rx(),
        Endpoint::Cache(index, CachePortId::Dev) => platform.caches[index].port_dev_rx(),
        Endpoint::Cache(index, CachePortId::Mem) => platform.caches[index].port_mem_rx(),
        Endpoint::Manager(index) => managers[index].port_rx(),
        Endpoint::Fabric(index, port) => platform.fabrics[index].port_ingress_i(port),
        Endpoint::Null => unreachable!("null endpoints use standalone ports"),
    };
    let connect = |endpoint, state: PortStateResult<MemoryAccess>| match endpoint {
        Endpoint::Pe(index) => platform.processing_elements[index].connect_port_tx(state),
        Endpoint::Memory(index) => platform.memories[index].connect_port_tx(state),
        Endpoint::Cache(index, CachePortId::Dev) => {
            platform.caches[index].connect_port_dev_tx(state)
        }
        Endpoint::Cache(index, CachePortId::Mem) => {
            platform.caches[index].connect_port_mem_tx(state)
        }
        Endpoint::Manager(index) => managers[index].connect_port_tx(state),
        Endpoint::Fabric(index, port) => platform.fabrics[index].connect_port_egress_i(port, state),
        Endpoint::Null => unreachable!("null endpoints use standalone ports"),
    };
    for &[left, right] in connections {
        if left == Endpoint::Null || right == Endpoint::Null {
            let endpoint = if left == Endpoint::Null { right } else { left };
            let (entity, tx_name, rx_name) = match endpoint {
                Endpoint::Fabric(index, port) => (
                    platform.fabrics[index].entity(),
                    format!("null_tx_{port}"),
                    format!("null_rx_{port}"),
                ),
                Endpoint::Cache(index, port) => (
                    platform.caches[index].entity(),
                    format!("null_{port}_tx"),
                    format!("null_{port}_rx"),
                ),
                _ => unreachable!("validated null peer"),
            };
            OutPort::new(entity, &tx_name).connect(receive(endpoint))?;
            let rx = InPort::new(engine, clock, entity, &rx_name);
            connect(endpoint, rx.state())?;
        } else {
            connect(left, receive(right))?;
            connect(right, receive(left))?;
        }
    }
    Ok(())
}
