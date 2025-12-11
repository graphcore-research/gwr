// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use serde::Deserialize;

#[derive(Debug, Clone, Copy)]
pub struct ComputeTaskConfig {
    pub op: ComputeOp,
    pub num_bytes: usize,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComputeOp {
    Add,
    Mul,
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryTaskConfig {
    pub op: MemoryOp,
    pub addr: u64,
    pub num_bytes: usize,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryOp {
    Load,
    Store,
}

#[derive(Debug, Clone, Copy)]
pub enum SyncRegion {
    Local,
    Global,
}

#[derive(Debug, Clone, Copy)]
pub enum Task {
    ComputeTask { config: ComputeTaskConfig },
    MemoryTask { config: MemoryTaskConfig },
    SyncTask { region: SyncRegion },
}
