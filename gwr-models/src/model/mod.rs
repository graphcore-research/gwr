// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Model-neutral logical ports and legacy platform reporting interfaces.

use std::collections::HashMap;
use std::rc::Rc;

use gwr_track::entity::{Entity, GetEntity};

use crate::cache::Cache;
use crate::fabric::{Fabric, FabricPortSelection};
use crate::memory::Memory;
use crate::memory::memory_access::MemoryAccess;

mod endpoint;
pub use endpoint::MemoryEndpoint;

pub trait MemoryModel: GetEntity {
    fn endpoint(&self) -> MemoryEndpoint<'_>;
    fn base_address(&self) -> u64;
    fn capacity_bytes(&self) -> usize;
    fn bytes_read(&self) -> usize;
    fn bytes_written(&self) -> usize;
    fn dump_stats(&self, time_now_ns: f64);
}

pub trait CacheModel: GetEntity {
    fn device_endpoint(&self) -> MemoryEndpoint<'_>;
    fn memory_endpoint(&self) -> MemoryEndpoint<'_>;
    fn payload_bytes_read(&self) -> usize;
    fn payload_bytes_written(&self) -> usize;
    fn num_hits(&self) -> usize;
    fn num_misses(&self) -> usize;
    fn dump_stats(&self, time_now_ns: f64);
}

pub trait FabricModel: GetEntity {
    fn endpoint(&self, port: usize) -> MemoryEndpoint<'_>;
    fn col_row_port_to_fabric_port_index(&self, col: usize, row: usize, port: usize) -> usize;
    fn fabric_port_index_to_col_row_port(&self, port: usize) -> (usize, usize, usize);
    fn destination_port_map(&self) -> &HashMap<u64, Vec<usize>>;
    fn port_selection(&self) -> FabricPortSelection;
}

/// Bridges the existing detailed fabric trait object to logical-port metadata.
pub struct DetailedFabricModel(pub Rc<dyn Fabric<MemoryAccess>>);

impl GetEntity for DetailedFabricModel {
    fn entity(&self) -> &Rc<Entity> {
        self.0.entity()
    }
}

impl FabricModel for DetailedFabricModel {
    fn endpoint(&self, port: usize) -> MemoryEndpoint<'_> {
        MemoryEndpoint::detailed(
            move || self.0.port_ingress_i(port),
            move |p| self.0.connect_port_egress_i(port, p),
        )
    }
    fn col_row_port_to_fabric_port_index(&self, col: usize, row: usize, port: usize) -> usize {
        self.0.col_row_port_to_fabric_port_index(col, row, port)
    }
    fn fabric_port_index_to_col_row_port(&self, port: usize) -> (usize, usize, usize) {
        self.0.fabric_port_index_to_col_row_port(port)
    }
    fn destination_port_map(&self) -> &HashMap<u64, Vec<usize>> {
        self.0.destination_port_map()
    }
    fn port_selection(&self) -> FabricPortSelection {
        self.0.port_selection()
    }
}

impl MemoryModel for Memory<MemoryAccess> {
    fn endpoint(&self) -> MemoryEndpoint<'_> {
        MemoryEndpoint::detailed(|| self.port_rx(), |p| self.connect_port_tx(p))
    }
    fn base_address(&self) -> u64 {
        Memory::base_address(self)
    }
    fn capacity_bytes(&self) -> usize {
        Memory::capacity_bytes(self)
    }
    fn bytes_read(&self) -> usize {
        Memory::bytes_read(self)
    }
    fn bytes_written(&self) -> usize {
        Memory::bytes_written(self)
    }
    fn dump_stats(&self, time_now_ns: f64) {
        Memory::dump_stats(self, time_now_ns);
    }
}

impl CacheModel for Cache<MemoryAccess> {
    fn device_endpoint(&self) -> MemoryEndpoint<'_> {
        MemoryEndpoint::detailed(|| self.port_dev_rx(), |p| self.connect_port_dev_tx(p))
    }
    fn memory_endpoint(&self) -> MemoryEndpoint<'_> {
        MemoryEndpoint::detailed(|| self.port_mem_rx(), |p| self.connect_port_mem_tx(p))
    }
    fn payload_bytes_read(&self) -> usize {
        Cache::payload_bytes_read(self)
    }
    fn payload_bytes_written(&self) -> usize {
        Cache::payload_bytes_written(self)
    }
    fn num_hits(&self) -> usize {
        Cache::num_hits(self)
    }
    fn num_misses(&self) -> usize {
        Cache::num_misses(self)
    }
    fn dump_stats(&self, time_now_ns: f64) {
        Cache::dump_stats(self, time_now_ns);
    }
}
