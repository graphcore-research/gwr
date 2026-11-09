// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Checked conversion of physical peak rates into simulator resource units.
//!
//! This utility does not change the simulation clock. Use the same tick period
//! when building the platform and running its workloads. Memory and fabric
//! models use integer rates: choose a tick that represents them exactly rather
//! than silently truncating bandwidth. Compute retains fractional rates.

use gwr_engine::types::SimError;

/// Physical duration of a simulation tick, with picosecond precision.
#[derive(Clone, Copy, Debug)]
pub struct TickScale {
    picoseconds: u64,
}

impl TickScale {
    /// Construct a positive tick period; one nanosecond is 1000 picoseconds.
    pub fn from_picoseconds(picoseconds: u64) -> Result<Self, SimError> {
        if picoseconds == 0 {
            return Err(SimError("tick period must be positive".into()));
        }
        Ok(Self { picoseconds })
    }

    /// Convert logical operations per second to logical operations per tick.
    /// Reject nonfinite, nonpositive and underflowing/overflowing rates.
    pub fn operations_per_tick(self, operations_per_second: f64) -> Result<f64, SimError> {
        let rate = operations_per_second * (self.picoseconds as f64 / 1e12);
        if !operations_per_second.is_finite()
            || operations_per_second <= 0.0
            || !rate.is_finite()
            || rate <= 0.0
        {
            return Err(SimError(
                "operation rate must be positive, finite and representable".into(),
            ));
        }
        Ok(rate)
    }

    /// Convert FLOPs/second to logical operations/tick. A matrix MAC is two
    /// FLOPs; its logical operation rate is half the published FLOP rate.
    pub fn flops_per_tick(
        self,
        flops_per_second: f64,
        flops_per_operation: usize,
    ) -> Result<f64, SimError> {
        if flops_per_operation == 0 {
            return Err(SimError("FLOPs per operation must be positive".into()));
        }
        self.operations_per_tick(flops_per_second / flops_per_operation as f64)
    }

    /// Convert bytes/second to exactly representable integer bytes/tick.
    pub fn bytes_per_tick(self, bytes_per_second: u64) -> Result<usize, SimError> {
        self.integer_rate(bytes_per_second)
    }

    /// Convert directional bits/second to exactly representable bits/tick.
    /// The caller must resolve any bidirectional aggregate source convention.
    pub fn bits_per_tick(self, bits_per_second: u64) -> Result<usize, SimError> {
        self.integer_rate(bits_per_second)
    }

    fn integer_rate(self, per_second: u64) -> Result<usize, SimError> {
        let scaled = u128::from(per_second) * u128::from(self.picoseconds);
        if scaled == 0 || scaled % 1_000_000_000_000 != 0 {
            return Err(SimError(
                "bandwidth must be positive and exactly representable at this tick period".into(),
            ));
        }
        usize::try_from(scaled / 1_000_000_000_000)
            .map_err(|_| SimError("bandwidth per tick overflows usize".into()))
    }
}
