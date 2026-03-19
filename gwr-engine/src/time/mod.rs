// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Modules that model time within the simulations.

use byte_unit::{AdjustedByte, Byte, UnitType};

pub mod clock;
pub mod simtime;

// Convert a number of bytes to a binary-only unit (KiB, MiB, etc)
#[must_use]
pub fn compute_adjusted_value_and_rate(
    time_now_ns: f64,
    num_bytes: usize,
) -> (AdjustedByte, AdjustedByte) {
    let time_now_s = time_now_ns / (1000.0 * 1000.0 * 1000.0);
    let count = Byte::from_u64(num_bytes as u64).get_appropriate_unit(UnitType::Binary);
    let per_second = if time_now_s == 0.0 {
        Byte::from_f64(0.0).unwrap()
    } else {
        Byte::from_f64(num_bytes as f64 / time_now_s).unwrap()
    };
    let count_per_second = per_second.get_appropriate_unit(UnitType::Binary);
    (count, count_per_second)
}
