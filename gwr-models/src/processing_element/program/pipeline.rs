// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Explicit software scheduling for a bounded block pipeline.
use gwr_engine::types::{SimError, SimResult};
use serde::{Deserialize, Serialize};

/// Requested scheduling mode and complete reserved buffer counts.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlockPipelineSpec {
    /// Whether independent pipeline stages may overlap.
    #[serde(default)]
    pub mode: PipelineMode,
    /// Number of complete operand-chunk slots.
    pub input_buffers: usize,
    /// Number of completed output-tile slots.
    pub output_buffers: usize,
    /// Number of chunks permitted ahead of the current compute chunk.
    pub prefetch_distance: usize,
}

/// Serial baseline or dependency-constrained overlapping execution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineMode {
    /// Issue every stage in order.
    Serial,
    /// Permit independent stages to overlap.
    #[default]
    Overlap,
}

impl BlockPipelineSpec {
    /// Validate slot counts without imposing a hardware-specific count ceiling.
    pub fn validate(&self) -> SimResult {
        if self.input_buffers == 0 || self.output_buffers == 0 {
            return Err(SimError(
                "pipeline input_buffers and output_buffers must be positive".into(),
            ));
        }
        if self.prefetch_distance >= self.input_buffers {
            return Err(SimError(
                "pipeline prefetch_distance must be less than input_buffers".into(),
            ));
        }
        Ok(())
    }
}
