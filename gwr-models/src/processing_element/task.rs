// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::types::SimError;

use crate::processing_element::program::PreparedProgram;
use crate::processing_element::program::syntax::{ProgramSpec, TileSpec};
use crate::processing_element::program::validate::RegionBindings;
use crate::processing_element::tensors::TensorView;

#[derive(Debug, Clone)]
pub enum Task {
    /// Raw custom-dispatch input, checked against its PE before activation.
    ProgramTask {
        config: Box<ProgramTaskConfig>,
    },
    /// Retained preparation handle bound to one exact PE instance.
    PreparedProgram {
        task: std::rc::Rc<PreparedTask>,
    },
    SyncTask {
        region: SyncRegion,
    },
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

/// An immutable task prepared for one exact processing-element instance.
///
/// The shared program owns its bindings. Preparation checks touched addresses,
/// actual compute work and duration, and SRAM capacity before dispatch can
/// activate the task. A different PE instance cannot execute this handle, even
/// when it has the same name.
#[derive(Debug)]
pub struct PreparedTask {
    id: String,
    program: std::rc::Rc<super::program::PreparedProgram>,
    owner: std::rc::Rc<gwr_track::entity::Entity>,
    compute: super::compute::PreparedCompute,
    peak_sram_bytes: usize,
    traffic_bytes: (usize, usize),
}
impl PreparedTask {
    pub(super) fn prepare(
        id: String,
        program: std::rc::Rc<super::program::PreparedProgram>,
        owner: std::rc::Rc<gwr_track::entity::Entity>,
        lsu: &super::load_store_unit::LoadStoreUnit,
        model: &super::compute::ComputeModel,
        capacity: usize,
    ) -> Result<std::rc::Rc<Self>, gwr_engine::types::SimError> {
        for view in program
            .facts
            .bindings
            .inputs
            .iter()
            .chain(&program.facts.bindings.outputs)
        {
            lsu.validate_mapped_view(view)?;
        }
        let super::program::analysis::TaskAnalysis {
            compute,
            peak_sram_bytes,
            traffic_bytes,
        } = program.analyze(model)?;
        if peak_sram_bytes > capacity {
            return Err(gwr_engine::types::SimError(format!(
                "{id} tile requires {peak_sram_bytes} SRAM bytes, available {capacity}"
            )));
        }
        Ok(std::rc::Rc::new(Self {
            id,
            program,
            owner,
            compute,
            peak_sram_bytes,
            traffic_bytes,
        }))
    }
    pub(super) fn belongs_to(&self, owner: &std::rc::Rc<gwr_track::entity::Entity>) -> bool {
        std::rc::Rc::ptr_eq(&self.owner, owner)
    }
    pub(super) fn lower(
        &self,
        work: &super::compute::ComputeWork,
    ) -> Result<super::compute::ExecutionPlan, gwr_engine::types::SimError> {
        self.compute.lower(work)
    }

    /// Task label used in activity traces.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Shared hardware-independent bound program.
    #[must_use]
    pub fn program(&self) -> &std::rc::Rc<super::program::PreparedProgram> {
        &self.program
    }
    /// Maximum live SRAM required by the task.
    #[must_use]
    pub fn peak_sram_bytes(&self) -> usize {
        self.peak_sram_bytes
    }
    /// Planned logical boundary read and write bytes, before LSU packetization.
    #[must_use]
    pub fn planned_traffic_bytes(&self) -> (usize, usize) {
        self.traffic_bytes
    }
}

impl ProgramTaskConfig {
    /// Bind and estimate boundary traffic without issuing requests.
    pub fn planned_traffic_bytes(&self) -> Result<(usize, usize), SimError> {
        PreparedProgram::new(
            &self.program,
            RegionBindings {
                inputs: self.inputs.clone(),
                outputs: self.outputs.clone(),
            },
            self.tiling.as_ref(),
        )?
        .planned_traffic_bytes()
    }
}
