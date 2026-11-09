// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Dense NCHW contraction lowering. Recognition uses expression structure and
//! indices, never workload names; all operands come from completed LSU loads.

use std::ops::Range;

use gwr_engine::types::{SimError, SimResult};

use super::functional::element_address;
use super::program::syntax::{AssignmentSpec, BoundsMode, ExprSpec, IndexSpec, ReductionOp};
use super::program::tile::RegionTile;
use super::program::validate::ValidatedRegion;
use super::task::ProgramTaskConfig;
use super::tensors::{HasShape, TensorView};
use crate::memory::FunctionalContext;

/// Return false when the generic scalar evaluator must handle this expression.
pub(super) fn evaluate(
    context: &FunctionalContext,
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    tile: &RegionTile,
    assignment_index: usize,
) -> Result<bool, SimError> {
    if config.program.assignments.len() != 1 || assignment_index != 0 {
        return Ok(false);
    }
    let assignment = &config.program.assignments[0];
    let Some(reads) = recognize(assignment) else {
        return Ok(false);
    };
    let input = |name: &str| {
        config
            .program
            .inputs
            .iter()
            .position(|value| value == name)
            .map(|i| &config.inputs[i])
    };
    let (Some(a), Some(w)) = (input(reads.input), input(reads.weight)) else {
        return Ok(false);
    };
    let Some(output) = config
        .program
        .outputs
        .iter()
        .position(|name| name == &assignment.primary_result())
        .map(|i| &config.outputs[i])
    else {
        return Ok(false);
    };
    if a.num_dims() != 4 || w.num_dims() != 4 {
        return Ok(false);
    }
    let shape = a.shape().dims();
    let ws = w.shape().dims();
    let axes = &assignment.axes;
    // Singleton broadcast and aliases retain the scalar evaluator's semantics.
    // Exact contraction extents and disjoint output storage are required here.
    if region.extent(&axes[0]) != Some(shape[0])
        || region.extent(&axes[1]) != Some(ws[0])
        || region.extent(&reads.over[0]) != Some(shape[1])
        || shape[1] != ws[1]
        || region.extent(&reads.over[1]) != Some(ws[2])
        || region.extent(&reads.over[2]) != Some(ws[3])
        || output.shape().dims()[0] != shape[0]
        || output.shape().dims()[1] != ws[0]
        || [a, w].iter().any(|operand| overlaps(output, operand))
    {
        return Ok(false);
    }
    let ranges: [Range<usize>; 4] = std::array::from_fn(|i| {
        tile.output_intervals
            .get(&axes[i])
            .cloned()
            .unwrap_or(0..region.extent(&axes[i]).unwrap())
    });
    let rows = crop(
        &ranges[2],
        reads.stride[0],
        reads.offset[0],
        ws[2],
        shape[2],
    );
    let columns = crop(
        &ranges[3],
        reads.stride[1],
        reads.offset[1],
        ws[3],
        shape[3],
    );
    if rows.is_empty() || columns.is_empty() {
        return Ok(false);
    }
    let convolution = Convolution {
        a,
        w,
        output,
        ranges,
        rows,
        columns,
        stride: reads.stride,
        offset: reads.offset,
    };
    convolution.execute(context)?;
    Ok(true)
}

struct Convolution<'a> {
    a: &'a TensorView,
    w: &'a TensorView,
    output: &'a TensorView,
    ranges: [Range<usize>; 4],
    rows: Range<usize>,
    columns: Range<usize>,
    stride: [usize; 2],
    offset: [i64; 2],
}
impl Convolution<'_> {
    fn execute(&self, context: &FunctionalContext) -> SimResult {
        let width = self.ranges[3].len();
        for batch in self.ranges[0].clone() {
            let activation = self.load_activation(context, batch)?;
            for channel in self.ranges[1].clone() {
                let result = self.channel(context, &activation, channel)?;
                for (row, y) in self.ranges[2].clone().enumerate() {
                    context.local.write_f32(
                        element_address(self.output, &[batch, channel, y, self.ranges[3].start])?,
                        &result[row * width..(row + 1) * width],
                    )?;
                }
            }
        }
        Ok(())
    }

    /// A stride larger than its kernel leaves holes that the LSU never loads.
    /// Copy only sampled coordinates; unused holes in this host buffer stay
    /// zero.
    fn load_activation(
        &self,
        context: &FunctionalContext,
        batch: usize,
    ) -> Result<Vec<f32>, SimError> {
        let channels = self.a.shape().dims()[1];
        let kernel = self.w.shape().dims();
        let width = self.columns.len();
        let mut activation = vec![0.; channels * self.rows.len() * width];
        for channel in 0..channels {
            for row in self.rows.clone() {
                if self.stride[0] > kernel[2]
                    && !self.ranges[2].clone().any(|y| {
                        let start = y as i128 * self.stride[0] as i128 + self.offset[0] as i128;
                        row as i128 >= start && (row as i128) < start + kernel[2] as i128
                    })
                {
                    continue;
                }
                let destination = (channel * self.rows.len() + row - self.rows.start) * width;
                if self.stride[1] <= kernel[3] {
                    activation[destination..destination + width].copy_from_slice(
                        &context.local.read_f32(
                            element_address(self.a, &[batch, channel, row, self.columns.start])?,
                            width,
                        )?,
                    );
                } else {
                    for x in self.ranges[3].clone() {
                        for kx in 0..kernel[3] {
                            let column = x as i128 * self.stride[1] as i128
                                + self.offset[1] as i128
                                + kx as i128;
                            if (self.columns.start as i128..self.columns.end as i128)
                                .contains(&column)
                            {
                                activation[destination + column as usize - self.columns.start] =
                                    context.local.read_f32(
                                        element_address(
                                            self.a,
                                            &[batch, channel, row, column as usize],
                                        )?,
                                        1,
                                    )?[0];
                            }
                        }
                    }
                }
            }
        }
        Ok(activation)
    }

    fn channel(
        &self,
        context: &FunctionalContext,
        activation: &[f32],
        output_channel: usize,
    ) -> Result<Vec<f32>, SimError> {
        let kernel = self.w.shape().dims();
        let width = self.ranges[3].len();
        let mut result = vec![0.; self.ranges[2].len() * width];
        for input_channel in 0..self.a.shape().dims()[1] {
            for ky in 0..kernel[2] {
                let weights = context.local.read_f32(
                    element_address(self.w, &[output_channel, input_channel, ky, 0])?,
                    kernel[3],
                )?;
                for (kx, &weight) in weights.iter().enumerate() {
                    for (row, y) in self.ranges[2].clone().enumerate() {
                        let iy = y as i128 * self.stride[0] as i128
                            + self.offset[0] as i128
                            + ky as i128;
                        if !(self.rows.start as i128..self.rows.end as i128).contains(&iy) {
                            continue;
                        }
                        let source = (input_channel * self.rows.len() + iy as usize
                            - self.rows.start)
                            * self.columns.len();
                        let destination = &mut result[row * width..(row + 1) * width];
                        for (column, x) in self.ranges[3].clone().enumerate() {
                            let ix = x as i128 * self.stride[1] as i128
                                + self.offset[1] as i128
                                + kx as i128;
                            if (self.columns.start as i128..self.columns.end as i128).contains(&ix)
                            {
                                destination[column] +=
                                    activation[source + ix as usize - self.columns.start] * weight;
                            }
                        }
                    }
                }
            }
        }
        Ok(result)
    }
}

struct Reads<'a> {
    input: &'a str,
    weight: &'a str,
    over: &'a [String],
    stride: [usize; 2],
    offset: [i64; 2],
}
fn recognize(assignment: &AssignmentSpec) -> Option<Reads<'_>> {
    let axes = &assignment.axes;
    if axes.len() != 4 {
        return None;
    }
    let ExprSpec::Reduce {
        op: ReductionOp::Sum,
        over,
        value,
    } = &assignment.expr
    else {
        return None;
    };
    if over.len() != 3 {
        return None;
    }
    let ExprSpec::Call { op, args, .. } = value.as_ref() else {
        return None;
    };
    if op != "mul" {
        return None;
    }
    let [
        ExprSpec::Read {
            tensor: input,
            indices: ai,
            bounds: BoundsMode::Exclude,
        },
        ExprSpec::Read {
            tensor: weight,
            indices: wi,
            bounds: BoundsMode::RequireInRange,
        },
    ] = args.as_slice()
    else {
        return None;
    };
    if ai.len() != 4
        || wi.len() != 4
        || !axis(&ai[0], &axes[0])
        || !axis(&ai[1], &over[0])
        || !axis(&wi[0], &axes[1])
        || !axis(&wi[1], &over[0])
        || !axis(&wi[2], &over[1])
        || !axis(&wi[3], &over[2])
    {
        return None;
    }
    let (sy, py) = window(&ai[2], &axes[2], &over[1])?;
    let (sx, px) = window(&ai[3], &axes[3], &over[2])?;
    Some(Reads {
        input,
        weight,
        over,
        stride: [sy, sx],
        offset: [py, px],
    })
}
fn axis(index: &IndexSpec, name: &str) -> bool {
    matches!(index, IndexSpec::Axis(value) if value == name)
}
fn window(index: &IndexSpec, parallel: &str, reduction: &str) -> Option<(usize, i64)> {
    let IndexSpec::Affine { affine } = index else {
        return None;
    };
    if affine.terms.len() != 2 || affine.terms.get(reduction) != Some(&1) {
        return None;
    }
    Some((*affine.terms.get(parallel)?, affine.offset))
}
fn crop(
    output: &Range<usize>,
    stride: usize,
    offset: i64,
    kernel: usize,
    extent: usize,
) -> Range<usize> {
    let start = (output.start as i128 * stride as i128 + offset as i128).clamp(0, extent as i128);
    let end = ((output.end - 1) as i128 * stride as i128 + offset as i128 + kernel as i128)
        .clamp(0, extent as i128);
    start as usize..end as usize
}
fn overlaps(a: &TensorView, b: &TensorView) -> bool {
    let start = |view: &TensorView| u128::from(view.tensor().addr());
    start(a) < start(b) + b.tensor().num_bytes() as u128
        && start(b) < start(a) + a.tensor().num_bytes() as u128
}
