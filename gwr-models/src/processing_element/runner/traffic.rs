// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::types::SimError;

use super::{find_read, slice_axes, slice_view, window_chunk_access_ranges, window_input_view};
use crate::processing_element::program::syntax::{ExprSpec, IndexSpec, ReductionOp};
use crate::processing_element::program::tile::plan_tiles;
use crate::processing_element::program::validate::{RegionBindings, validate_program};
use crate::processing_element::task::ProgramTaskConfig;

impl ProgramTaskConfig {
    /// Boundary traffic implied by this node's tile schedule, including
    /// deliberate reloads across output and reduction tiles.
    #[expect(
        clippy::too_many_lines,
        reason = "Mirror execution variants while computing the traffic contract"
    )]
    pub fn planned_traffic_bytes(&self) -> Result<(usize, usize), SimError> {
        let region = validate_program(
            &self.program,
            &RegionBindings {
                inputs: self.inputs.clone(),
                outputs: self.outputs.clone(),
            },
        )?;
        let schedule = plan_tiles(&region, self.tiling.as_ref())?;
        let mut read_bytes = 0usize;
        let mut written_bytes = 0usize;
        for tile in schedule.tiles() {
            for (name, view) in self.program.inputs.iter().zip(&self.inputs) {
                if self.program.metadata_inputs.contains(name) {
                    let bytes = if self.program.indexed_metadata_inputs.contains(name) {
                        crate::processing_element::program::indexed_metadata::ranges(
                            &region, &tile, name,
                        )?
                        .1
                    } else {
                        view.layout().num_access_bytes()
                    };
                    read_bytes = read_bytes
                        .checked_add(bytes)
                        .ok_or_else(|| SimError("planned read traffic overflows".into()))?;
                    continue;
                }

                let mut reduction_uses = super::tile::is_reduction_input(self, name);
                for assignment in &self.program.assignments {
                    match &assignment.expr {
                        ExprSpec::Reduce {
                            op: ReductionOp::Sum,
                            over,
                            value,
                        } if find_read(value, name).is_some() => {
                            reduction_uses = true;
                            let indices = find_read(value, name).unwrap();
                            for chunk in tile.reduction_chunks_for(over) {
                                let bytes = if indices
                                    .iter()
                                    .any(|index| matches!(index, IndexSpec::Affine { .. }))
                                {
                                    window_chunk_access_ranges(
                                        view, indices, &tile, &region, &chunk,
                                    )?
                                    .1
                                } else {
                                    let mut intervals = tile.output_intervals.clone();
                                    intervals.extend(chunk);
                                    slice_view(view, indices, &intervals)?
                                        .layout()
                                        .num_access_bytes()
                                };
                                read_bytes = read_bytes.checked_add(bytes).ok_or_else(|| {
                                    SimError("planned read traffic overflows".into())
                                })?;
                            }
                        }
                        ExprSpec::MatrixMac { over, .. }
                            if find_read(&assignment.expr, name).is_some() =>
                        {
                            reduction_uses = true;
                            let indices = find_read(&assignment.expr, name).unwrap();
                            for chunk in tile.reduction_chunks_for(over) {
                                let mut intervals = tile.output_intervals.clone();
                                intervals.extend(chunk);
                                let bytes = slice_view(view, indices, &intervals)?
                                    .layout()
                                    .num_access_bytes();
                                read_bytes = read_bytes.checked_add(bytes).ok_or_else(|| {
                                    SimError("planned read traffic overflows".into())
                                })?;
                            }
                        }
                        ExprSpec::Reduce {
                            op: ReductionOp::Min | ReductionOp::Max,
                            over,
                            value,
                        } if find_read(value, name).is_some() => {
                            reduction_uses = true;
                            let indices = find_read(value, name).unwrap();
                            for chunk in tile.reduction_chunks_for(over) {
                                let (_, bytes) = window_chunk_access_ranges(
                                    view, indices, &tile, &region, &chunk,
                                )?;
                                read_bytes = read_bytes.checked_add(bytes).ok_or_else(|| {
                                    SimError("planned read traffic overflows".into())
                                })?;
                            }
                        }
                        ExprSpec::Call { .. } | ExprSpec::Read { .. }
                            if super::tile::is_reduction_input(self, name) =>
                        {
                            if let Some(indices) = find_read(&assignment.expr, name) {
                                let bytes = if indices
                                    .iter()
                                    .any(|index| matches!(index, IndexSpec::Affine { .. }))
                                {
                                    window_input_view(view, indices, &tile, &region)?
                                        .layout()
                                        .num_access_bytes()
                                } else {
                                    slice_view(view, indices, &tile.output_intervals)?
                                        .layout()
                                        .num_access_bytes()
                                };
                                read_bytes = read_bytes.checked_add(bytes).ok_or_else(|| {
                                    SimError("planned read traffic overflows".into())
                                })?;
                            }
                        }
                        _ => {}
                    }
                }
                if !reduction_uses {
                    let Some(indices) = self
                        .program
                        .assignments
                        .iter()
                        .find_map(|assignment| find_read(&assignment.expr, name))
                    else {
                        continue;
                    };
                    let slice = if indices
                        .iter()
                        .any(|index| matches!(index, IndexSpec::Affine { .. }))
                    {
                        window_input_view(view, indices, &tile, &region)?
                    } else {
                        slice_view(view, indices, &tile.output_intervals)?
                    };
                    read_bytes = read_bytes
                        .checked_add(slice.layout().num_access_bytes())
                        .ok_or_else(|| SimError("planned read traffic overflows".into()))?;
                }
            }
            for (name, view) in self.program.outputs.iter().zip(&self.outputs) {
                let assignment = self
                    .program
                    .assignments
                    .iter()
                    .find(|assignment| {
                        &assignment.primary_result() == name
                            || assignment.indices_output.as_ref() == Some(name)
                    })
                    .ok_or_else(|| SimError(format!("missing assignment for {name}")))?;
                let bytes = slice_axes(view, &assignment.axes, &tile.output_intervals)?
                    .layout()
                    .num_access_bytes();
                written_bytes = written_bytes
                    .checked_add(bytes)
                    .ok_or_else(|| SimError("planned write traffic overflows".into()))?;
            }
        }
        Ok((read_bytes, written_bytes))
    }
}
