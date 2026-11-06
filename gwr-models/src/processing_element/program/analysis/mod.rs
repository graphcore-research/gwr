// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Pure invocation, access and storage analysis shared with task execution.
//! Complete preparation consumes each lazy invocation once for compute and
//! traffic. Standalone estimates retain their cheaper, focused paths.

use std::collections::BTreeMap;

use gwr_engine::types::{SimError, SimResult};

use self::access::{
    InputRead, build_tile_output_views, collect_assignment_input_reads, collect_initial_input_reads,
};
use self::invocation::AssignmentInvocations;
use self::tile_storage::{build_tile_storage, estimate_tile_sram};
use super::PreparedProgram;
use super::executable::Expr;
use super::prepared::{GroupPlan, Schedule};
use super::syntax::{BoundsMode, IndexSpec, ProgramSpec, ReductionOp, TileSpec};
use super::tile::RegionTile;
use super::validate::RegionBindings;
use crate::processing_element::MachineOpCounts;
use crate::processing_element::compute::{ComputeModel, ComputeWork, PreparedCompute};
use crate::processing_element::tensors::TensorView;

pub(in crate::processing_element) mod access;
mod helpers;
pub(in crate::processing_element) mod invocation;
pub(in crate::processing_element) mod matrix_shape;
pub(in crate::processing_element) mod tile_storage;
mod window;

pub(in crate::processing_element) struct TaskAnalysis {
    pub compute: PreparedCompute,
    pub peak_sram_bytes: usize,
    pub traffic_bytes: (usize, usize),
}

impl PreparedProgram {
    /// Prepare compute, storage and traffic together without retaining tiles.
    pub(in crate::processing_element) fn analyze(
        &self,
        model: &ComputeModel,
    ) -> Result<TaskAnalysis, SimError> {
        // A proven maximum flat tile needs only one storage plan, even though
        // compute and traffic must inspect every invocation and clipped tail.
        let maximum_tile_sram = self.maximum_tile_sram()?;
        let mut peak_sram_bytes = maximum_tile_sram.unwrap_or(0);
        let mut compute = PreparedCompute::default();
        let mut counts = MachineOpCounts::default();
        let mut read_bytes = 0;
        let mut written_bytes = 0;
        self.visit_groups(&mut |plan, tile, retained| {
            let outputs = if maximum_tile_sram.is_some() {
                build_tile_output_views(plan, tile)?
            } else {
                let storage = build_tile_storage(plan, tile, usize::MAX)?;
                peak_sram_bytes = peak_sram_bytes.max(group_sram_bytes(retained, storage.arena)?);
                storage.outputs
            };
            analyze_group_reads(plan, tile, &mut read_bytes, |work| {
                counts = counts.clone().checked_add(compute.prepare(model, work)?)?;
                Ok(())
            })?;
            add_write_traffic(&mut written_bytes, &outputs)
        })?;
        Ok(TaskAnalysis {
            compute,
            peak_sram_bytes,
            traffic_bytes: (read_bytes, written_bytes),
        })
    }

    /// Maximum live storage across the canonical hardware-independent schedule.
    /// Direct slices and suitable windows need only a maximum-tile probe. Other
    /// cases traverse lazy tiles using bounded memory.
    pub fn peak_sram_bytes(&self) -> Result<usize, SimError> {
        if let Some(peak) = self.maximum_tile_sram()? {
            return Ok(peak);
        }
        let mut peak = 0;
        self.visit_groups(&mut |plan, tile, retained| {
            peak = peak.max(group_sram_bytes(retained, estimate_tile_sram(plan, tile)?)?);
            Ok(())
        })?;
        Ok(peak)
    }

    /// Hardware-independent boundary traffic from the canonical lazy schedule.
    /// Analysis uses bounded memory without collecting every invocation;
    /// traversal time can still scale with invocation count.
    pub fn planned_traffic_bytes(&self) -> Result<(usize, usize), SimError> {
        let mut read_bytes = 0;
        let mut written_bytes = 0;
        self.visit_groups(&mut |plan, tile, _| {
            analyze_group_reads(plan, tile, &mut read_bytes, |_| Ok(()))?;
            add_write_traffic(&mut written_bytes, &build_tile_output_views(plan, tile)?)
        })?;
        Ok((read_bytes, written_bytes))
    }

    fn maximum_tile_sram(&self) -> Result<Option<usize>, SimError> {
        // Preserve the bounded maximum-tile proof for direct slices and
        // nonnegative byte windows. Leading padding and packed reductions
        // require offset-sensitive inspection.
        let Schedule::Flat { tiles, group } = &self.schedule else {
            return Ok(None);
        };
        let packed = self
            .facts
            .bindings
            .inputs
            .iter()
            .any(|view| view.dtype().num_bits() < 8);
        let inspect_all = self.facts.program.assignments.iter().any(|assignment| {
            let leading = assignment.expr.iter_reads().any(|read| {
                read.bounds == BoundsMode::Exclude
                    && read.indices.iter().any(
                        |index| matches!(index, IndexSpec::Affine { affine } if affine.offset < 0),
                    )
            });
            let window = match &assignment.expr {
                Expr::Reduce {
                    op: ReductionOp::Min | ReductionOp::Max,
                    ..
                } => true,
                Expr::Reduce { .. } => assignment.expr.iter_reads().any(|read| {
                    read.indices
                        .iter()
                        .any(|index| matches!(index, IndexSpec::Affine { .. }))
                }),
                _ => false,
            };
            leading || (packed && window)
        });
        if inspect_all {
            return Ok(None);
        }
        let tile = tiles
            .tiles()
            .next()
            .ok_or_else(|| SimError("program has no tiles".into()))?;
        estimate_tile_sram(self.group(group), &tile).map(Some)
    }
}

/// Count each group's boundary reads while consuming its lazy invocations.
/// Complete preparation lowers the same work here; traffic-only callers do
/// not require a hardware model or prepared compute state.
fn analyze_group_reads(
    plan: GroupPlan<'_>,
    tile: &RegionTile,
    read_bytes: &mut usize,
    mut prepare_work: impl FnMut(&ComputeWork) -> SimResult,
) -> SimResult {
    let mut add_read = |read: &InputRead<'_>, chunk: &BTreeMap<_, _>| {
        *read_bytes = read_bytes
            .checked_add(read.plan_access(tile, plan.region(), chunk)?.bytes())
            .ok_or_else(|| SimError("planned read traffic overflows".into()))?;
        Ok::<_, SimError>(())
    };
    for read in collect_initial_input_reads(plan) {
        add_read(&read, &BTreeMap::new())?;
    }
    for &index in &plan.group.assignments {
        let reads = collect_assignment_input_reads(plan, index);
        for invocation in AssignmentInvocations::new(plan.region(), index, tile)? {
            let invocation = invocation?;
            prepare_work(&invocation.work)?;
            if invocation.read_label.is_some() {
                for read in &reads {
                    add_read(read, &invocation.chunk)?;
                }
            }
        }
    }
    Ok(())
}

fn add_write_traffic(written_bytes: &mut usize, outputs: &[(&str, TensorView)]) -> SimResult {
    for (_, view) in outputs {
        *written_bytes = written_bytes
            .checked_add(view.layout().num_access_bytes())
            .ok_or_else(|| SimError("planned write traffic overflows".into()))?;
    }
    Ok(())
}

fn group_sram_bytes(retained: usize, tile: usize) -> Result<usize, SimError> {
    retained
        .checked_add(tile)
        .ok_or_else(|| SimError("schedule working set overflows".into()))
}

/// Bind a raw program and compute the maximum live tile storage.
pub fn estimate_peak_tile_sram(
    program: &ProgramSpec,
    inputs: &[TensorView],
    outputs: &[TensorView],
    tiling: &TileSpec,
) -> Result<usize, SimError> {
    PreparedProgram::new(
        program,
        RegionBindings {
            inputs: inputs.to_vec(),
            outputs: outputs.to_vec(),
        },
        Some(tiling),
    )?
    .peak_sram_bytes()
}

impl ComputeModel {
    /// Check actual invocation counts and geometry without binding to a PE.
    pub fn validate_program(&self, program: &PreparedProgram) -> SimResult {
        let mut compute = PreparedCompute::default();
        let mut counts = MachineOpCounts::default();
        program.visit_groups(&mut |plan, tile, _| {
            for &index in &plan.group.assignments {
                for invocation in AssignmentInvocations::new(plan.region(), index, tile)? {
                    counts = counts
                        .clone()
                        .checked_add(compute.prepare(self, &invocation?.work)?)?;
                }
            }
            Ok(())
        })
    }
}
