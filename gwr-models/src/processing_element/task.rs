// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;
use gwr_track::entity::Entity;

use crate::processing_element::compute::{
    ComputeModel, ComputeWork, ExecutionPlan, PreparedCompute,
};
use crate::processing_element::load_store_unit::LoadStoreUnit;
use crate::processing_element::program::analysis::TaskAnalysis;
use crate::processing_element::program::syntax::{ProgramSpec, TileSpec};
use crate::processing_element::program::{PreparedProgram, ProgramBindings};
use crate::processing_element::tensors::TensorView;

#[derive(Debug, Clone)]
pub enum Task {
    /// Raw custom-dispatch input, checked against its PE before activation.
    ProgramTask {
        /// Owned syntax, tiling and boundary views to prepare before dispatch.
        config: Box<ProgramTaskConfig>,
    },
    /// Retained preparation handle bound to one exact PE instance.
    PreparedProgram {
        /// Preparation retained for the exact PE instance that created it.
        task: Rc<PreparedTask>,
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

/// Raw syntax and owned views for a custom dispatcher.
///
/// Inputs and outputs follow the program's declaration order. Use
/// [`super::ProcessingElement::prepare_program_task`] to retain a checked
/// handle before dispatch, or return [`Task::ProgramTask`] and let the PE
/// prepare it before activation. Use [`PreparedProgram`] when sharing the bound
/// program between hardware-independent analysis and PE preparation.
#[derive(Debug, Clone)]
pub struct ProgramTaskConfig {
    /// Task label used in activity traces and preparation errors.
    pub id: String,
    /// Named tensor program to bind and validate.
    pub program: ProgramSpec,
    /// Optional axis chunk sizes; omitted axes use their resolved extents.
    pub tiling: Option<TileSpec>,
    /// Input views in the program's declaration order.
    pub inputs: Vec<TensorView>,
    /// Output views in the program's declaration order.
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
    program: Rc<PreparedProgram>,
    owner: Rc<Entity>,
    compute: PreparedCompute,
    peak_sram_bytes: usize,
    traffic_bytes: (usize, usize),
}

impl PreparedTask {
    pub(super) fn prepare(
        id: String,
        program: Rc<PreparedProgram>,
        owner: Rc<Entity>,
        lsu: &LoadStoreUnit,
        model: &ComputeModel,
        capacity: usize,
    ) -> Result<Rc<Self>, SimError> {
        for view in program
            .facts
            .bindings
            .inputs
            .iter()
            .chain(&program.facts.bindings.outputs)
        {
            lsu.validate_mapped_view(view)?;
        }
        let TaskAnalysis {
            compute,
            peak_sram_bytes,
            traffic_bytes,
        } = program.analyze(model)?;
        if peak_sram_bytes > capacity {
            return sim_error!(
                "{id} tile requires {peak_sram_bytes} SRAM bytes, available {capacity}"
            );
        }
        Ok(Rc::new(Self {
            id,
            program,
            owner,
            compute,
            peak_sram_bytes,
            traffic_bytes,
        }))
    }

    pub(super) fn belongs_to(&self, owner: &Rc<Entity>) -> bool {
        Rc::ptr_eq(&self.owner, owner)
    }

    pub(super) fn lower(&self, work: &ComputeWork) -> Result<ExecutionPlan, SimError> {
        self.compute.lower(work)
    }

    /// Task label used in activity traces.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Shared hardware-independent bound program.
    #[must_use]
    pub fn program(&self) -> &Rc<PreparedProgram> {
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
            ProgramBindings {
                inputs: self.inputs.clone(),
                outputs: self.outputs.clone(),
            },
            self.tiling.as_ref(),
        )?
        .planned_traffic_bytes()
    }
}
