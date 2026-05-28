// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};

#[derive(Clone)]
pub struct CacheConfig {
    pub(super) line_size_bytes: usize,
    pub(super) max_inflight_parents: usize,
    pub(super) max_inflight_fills: usize,
    pub(super) bw_bytes_per_tick: usize,
    pub(super) num_sets: usize,
    pub(super) num_ways: usize,
    pub(super) delay_ticks: usize,
}

impl CacheConfig {
    #[must_use]
    pub fn new(
        line_size_bytes: usize,
        bw_bytes_per_tick: usize,
        num_sets: usize,
        num_ways: usize,
        delay_ticks: usize,
    ) -> Self {
        Self {
            line_size_bytes,
            max_inflight_parents: 64,
            max_inflight_fills: 64,
            bw_bytes_per_tick,
            num_sets,
            num_ways,
            delay_ticks,
        }
    }

    /// Bytes fetched by each downstream read miss.
    #[must_use]
    pub fn line_size_bytes(&self) -> usize {
        self.line_size_bytes
    }

    /// Set separate limits on tracked original reads (parents) and downstream
    /// line reads awaiting responses (fills). A parent remains tracked until
    /// all its required lines have been processed and its pending fills
    /// complete. Admission remains occupied through response service in
    /// coherent and incoherent caches. Larger valid requests stream
    /// through these bounds.
    #[must_use]
    pub fn with_admission_limits(
        mut self,
        max_inflight_parents: usize,
        max_inflight_fills: usize,
    ) -> Self {
        self.max_inflight_parents = max_inflight_parents;
        self.max_inflight_fills = max_inflight_fills;
        self
    }

    pub fn validate(&self) -> SimResult {
        if self.line_size_bytes == 0 {
            return sim_error!("cache line size must be greater than zero");
        }
        if self.max_inflight_parents == 0 || self.max_inflight_fills == 0 {
            return sim_error!("cache admission bounds must be positive");
        }
        if self.bw_bytes_per_tick == 0 {
            return sim_error!("cache bandwidth must be greater than zero");
        }
        if self.num_sets == 0 {
            return sim_error!("cache set count must be greater than zero");
        }
        if self.num_ways == 0 {
            return sim_error!("cache way count must be greater than zero");
        }
        self.num_sets
            .checked_mul(self.num_ways)
            .and_then(|n| n.checked_mul(self.line_size_bytes))
            .ok_or_else(|| SimError("cache entry count overflows".to_string()))?;
        Ok(())
    }
}
