// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use byte_unit::{AdjustedByte, Byte, UnitType};
use gwr_engine::time::clock::Clock;
use gwr_models::processing_element::operators::MachineOps;
use gwr_models::processing_element::task::ComputeOp;

pub mod memory;
pub mod pe;

#[derive(Clone, Debug)]
pub struct ComputeNodeAnalysis {
    pub node_idx: usize,
    pub id: String,
    pub pe_name: Option<String>,
    pub op: ComputeOp,
    pub machine_ops: MachineOps,
    pub flops: usize,
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub bytes_by_memory: BTreeMap<String, usize>,
    pub tensor_access_addrs: Vec<u64>,
    pub predecessor_compute_node_indices: Vec<usize>,
    pub predecessor_compute_node_ids: Vec<String>,
}

impl ComputeNodeAnalysis {
    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.input_bytes + self.output_bytes
    }

    #[must_use]
    pub fn flops_per_byte(&self) -> f64 {
        if self.total_bytes() == 0 {
            0.0
        } else {
            self.flops as f64 / self.total_bytes() as f64
        }
    }
}

#[must_use]
pub fn ticks_to_ns(clock: &Clock, ticks: f64) -> f64 {
    ticks / clock.freq_mhz() * 1000.0
}

#[must_use]
pub fn format_bytes(bytes: usize) -> AdjustedByte {
    Byte::from_u64(bytes as u64).get_appropriate_unit(UnitType::Binary)
}

#[must_use]
pub fn bytes_per_tick_to_gb_per_s(clock: &Clock, bytes_per_tick: f64) -> f64 {
    bytes_per_tick * clock.freq_mhz() / 1000.0
}
