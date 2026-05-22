// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use byte_unit::{AdjustedByte, Byte, UnitType};
use gwr_engine::time::clock::Clock;
use gwr_models::processing_element::MachineOpCounts;
use gwr_models::processing_element::task::ComputeOp;

pub mod memory;
pub mod pe;
pub mod report;
pub mod roofline;

const SI_UNITS: &[&str] = &["", "K", "M", "G", "T", "P", "E"];

#[derive(Clone, Debug, PartialEq)]
pub struct AdjustedValueAndRate {
    pub value: f64,
    pub value_unit: String,
    pub rate: f64,
    pub rate_unit: String,
}

impl std::fmt::Display for AdjustedValueAndRate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:.2} {}, {:.2} {}/s",
            self.value, self.value_unit, self.rate, self.rate_unit
        )
    }
}

#[derive(Clone, Debug)]
pub struct ComputeNodeAnalysis {
    pub node_idx: usize,
    pub id: String,
    pub pe_name: Option<String>,
    pub op: ComputeOp,
    pub machine_ops: MachineOpCounts,
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

#[must_use]
pub fn compute_adjusted_value_and_rate(
    value: f64,
    duration_seconds: f64,
    base_unit: &str,
) -> AdjustedValueAndRate {
    let (adjusted_value, value_prefix) = adjust_si_value(value);
    let rate = if duration_seconds > 0.0 {
        value / duration_seconds
    } else {
        0.0
    };
    let (adjusted_rate, rate_prefix) = adjust_si_value(rate);

    AdjustedValueAndRate {
        value: adjusted_value,
        value_unit: format!("{value_prefix}{base_unit}"),
        rate: adjusted_rate,
        rate_unit: format!("{rate_prefix}{base_unit}"),
    }
}

fn adjust_si_value(value: f64) -> (f64, &'static str) {
    if value == 0.0 || !value.is_finite() {
        return (value, SI_UNITS[0]);
    }

    let mut adjusted = value.abs();
    let mut unit_idx = 0;
    while adjusted >= 1000.0 && unit_idx + 1 < SI_UNITS.len() {
        adjusted /= 1000.0;
        unit_idx += 1;
    }

    (value.signum() * adjusted, SI_UNITS[unit_idx])
}

#[cfg(test)]
mod tests {
    use super::compute_adjusted_value_and_rate;

    #[test]
    fn adjusted_value_and_rate_scales_value_and_rate() {
        let adjusted = compute_adjusted_value_and_rate(7_100_000.0, 0.000_043, "FLOP");

        assert_eq!(adjusted.value_unit, "MFLOP");
        assert!((adjusted.value - 7.1).abs() < 0.001);
        assert_eq!(adjusted.rate_unit, "GFLOP");
        assert!((adjusted.rate - 165.116).abs() < 0.001);
    }
}
