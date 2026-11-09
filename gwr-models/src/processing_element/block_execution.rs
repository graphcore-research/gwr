// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Platform-owned limits and storage backing for bounded block execution.
use gwr_engine::types::{SimError, SimResult};
use serde::{Deserialize, Serialize};

/// PE-wide active stage limits, independent of software buffer counts.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlockExecutionConfig {
    /// Maximum live compute/accumulator contexts. V1 requires one.
    pub max_compute_tiles: usize,
    /// Maximum simultaneous operand-chunk load jobs. V1 requires one.
    pub max_load_chunks: usize,
    /// Maximum simultaneous output-tile store jobs. V1 requires one.
    pub max_store_tiles: usize,
    /// Connected transfer service used for external traffic.
    pub transfer_port: BlockTransferPort,
    /// Physical backing for retained accumulators.
    pub accumulator_storage: AccumulatorStorage,
    /// Optional caller-qualified combined-direction local staging rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_memory_service: Option<LocalStagingConfig>,
}

/// External transfer route supported by block execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockTransferPort {
    /// The PE's real load/store unit.
    Lsu,
}

/// Storage charged to the complete pipeline working window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccumulatorStorage {
    /// Retained state occupies the shared PE SRAM pool.
    SharedSram,
}

/// Combined-direction staging bytes per simulation tick.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalStagingConfig {
    /// Positive staging service rate.
    pub bytes_per_tick: u64,
}

impl BlockExecutionConfig {
    /// Reject unsupported concurrency and non-positive selected service rates.
    pub fn validate(&self) -> SimResult {
        if self.max_compute_tiles != 1 || self.max_load_chunks != 1 || self.max_store_tiles != 1 {
            return Err(SimError("block_execution V1 requires max_compute_tiles, max_load_chunks and max_store_tiles to equal 1".into()));
        }
        if self
            .local_memory_service
            .as_ref()
            .is_some_and(|service| service.bytes_per_tick == 0)
        {
            return Err(SimError(
                "block_execution local_memory_service bytes_per_tick must be positive".into(),
            ));
        }
        Ok(())
    }
}
