// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Dense contractions packed once from actual LSU data, not interpreted per
//! MAC.

use std::ops::Range;

use gwr_engine::types::SimError;

use super::functional::element_address;
use super::program::syntax::{BoundsMode, ExprSpec, IndexSpec, ReductionOp};
use super::program::tile::RegionTile;
use super::program::validate::ValidatedRegion;
use super::task::ProgramTaskConfig;
use super::tensors::{HasShape, TensorView};
use crate::memory::FunctionalContext;

/// Unsupported expressions retain the ordinary scalar evaluator.
pub(super) fn evaluate(
    context: &FunctionalContext,
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    tile: &RegionTile,
    assignment_index: usize,
) -> Result<bool, SimError> {
    let assignment = &config.program.assignments[assignment_index];
    let [row_axis, column_axis] = assignment.axes.as_slice() else {
        return Ok(false);
    };
    let (args, over) = match &assignment.expr {
        ExprSpec::MatrixMac { args, over, .. } => (args, over),
        ExprSpec::Reduce {
            op: ReductionOp::Sum,
            over,
            value,
        } => {
            let ExprSpec::Call { op, args, .. } = value.as_ref() else {
                return Ok(false);
            };
            if op != "mul" {
                return Ok(false);
            }
            (args, over)
        }
        _ => return Ok(false),
    };
    let ([left, right], [reduction]) = (args.as_slice(), over.as_slice()) else {
        return Ok(false);
    };
    let (Some(a), Some(b)) = (
        Operand::recognize(left, config, row_axis, reduction),
        Operand::recognize(right, config, reduction, column_axis),
    ) else {
        return Ok(false);
    };
    let rows = tile
        .output_intervals
        .get(row_axis)
        .cloned()
        .unwrap_or(0..region.extent(row_axis).unwrap());
    let columns = tile
        .output_intervals
        .get(column_axis)
        .cloned()
        .unwrap_or(0..region.extent(column_axis).unwrap());
    let k = region.extent(reduction).unwrap();
    // Both operands are snapshotted before any result is published. Missing
    // logical bytes remain errors; packet padding never becomes an operand.
    let left = a.pack(context, &rows, &(0..k))?;
    let right = b.pack(context, &(0..k), &columns)?;
    let values = super::functional_matmul::multiply(rows.len(), columns.len(), k, &left, &right)
        .map_err(|message| SimError(message.into()))?;
    let name = assignment.primary_result();
    let output = config
        .program
        .outputs
        .iter()
        .position(|value| value == &name)
        .map(|index| &config.outputs[index]);
    for (index, row) in rows.enumerate() {
        let values = &values[index * columns.len()..(index + 1) * columns.len()];
        if let Some(output) = output {
            context
                .local
                .write_f32(element_address(output, &[row, columns.start])?, values)?;
        }
        if config.program.assignments.len() > 1 {
            let mut locals = context.values.borrow_mut();
            for (column, &value) in columns.clone().zip(values) {
                locals.insert(
                    (name.clone(), vec![row, column]),
                    crate::memory::FunctionalValue::from_f32(
                        super::tensors::dtype::DataType::Fp32,
                        value,
                    )?,
                );
            }
        }
    }
    Ok(true)
}

struct Operand<'a> {
    view: &'a TensorView,
    transposed: bool,
}

impl<'a> Operand<'a> {
    fn recognize(
        expr: &ExprSpec,
        config: &'a ProgramTaskConfig,
        first: &str,
        second: &str,
    ) -> Option<Self> {
        let ExprSpec::Read {
            tensor,
            indices,
            bounds: BoundsMode::RequireInRange,
        } = expr
        else {
            return None;
        };
        let [IndexSpec::Axis(left), IndexSpec::Axis(right)] = indices.as_slice() else {
            return None;
        };
        let transposed = if left == first && right == second {
            false
        } else if left == second && right == first {
            true
        } else {
            return None;
        };
        let index = config
            .program
            .inputs
            .iter()
            .position(|name| name == tensor)?;
        let view = &config.inputs[index];
        (view.num_dims() == 2).then_some(Self { view, transposed })
    }

    fn pack(
        &self,
        context: &FunctionalContext,
        first: &Range<usize>,
        second: &Range<usize>,
    ) -> Result<Vec<f32>, SimError> {
        let capacity = first
            .len()
            .checked_mul(second.len())
            .ok_or_else(|| SimError("functional GEMM packing dimensions overflow".into()))?;
        let (rows, columns) = if self.transposed {
            (second, first)
        } else {
            (first, second)
        };
        let shape = self.view.shape().dims();
        let physical_rows = if shape[0] == 1 { 0..1 } else { rows.clone() };
        let physical_columns = if shape[1] == 1 { 0..1 } else { columns.clone() };
        let storage_capacity = physical_rows
            .len()
            .checked_mul(physical_columns.len())
            .ok_or_else(|| {
                SimError("functional GEMM physical packing dimensions overflow".into())
            })?;
        let mut storage = Vec::with_capacity(storage_capacity);
        for row in physical_rows {
            storage.extend(context.local.read_f32(
                element_address(self.view, &[row, physical_columns.start])?,
                physical_columns.len(),
            )?);
        }
        if !self.transposed && shape[0] != 1 && shape[1] != 1 {
            return Ok(storage);
        }
        let mut packed = Vec::with_capacity(capacity);
        for row in 0..first.len() {
            for column in 0..second.len() {
                let (r, c) = if self.transposed {
                    (column, row)
                } else {
                    (row, column)
                };
                let r = if shape[0] == 1 { 0 } else { r };
                let c = if shape[1] == 1 { 0 } else { c };
                packed.push(storage[r * physical_columns.len() + c]);
            }
        }
        Ok(packed)
    }
}
