// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use crate::processing_element::program::syntax::{ProgramSpec, TileSpec};
use crate::processing_element::tensors::TensorView;

#[derive(Debug, Clone)]
pub enum Task {
    ProgramTask { config: Box<ProgramTaskConfig> },
    SyncTask { region: SyncRegion },
}

#[derive(Debug, Clone, Copy)]
pub enum SyncRegion {
    Local,
    Global,
}

#[derive(Debug, Clone)]
pub struct ProgramTaskConfig {
    pub id: String,
    pub program: ProgramSpec,
    pub tiling: Option<TileSpec>,
    pub inputs: Vec<TensorView>,
    pub outputs: Vec<TensorView>,
}
