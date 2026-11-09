// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Matrix role derivation shared by preflight and actual chunks. Shared batch
//! axes are supported when each resolved chunk selects one batch coordinate.
use std::collections::BTreeMap;
use std::ops::Range;

use gwr_engine::types::SimError;

use crate::processing_element::compute::{ComputeModel, ComputeWork, MatrixWorkShape};
use crate::processing_element::program::syntax::{AssignmentSpec, BoundsMode, ExprSpec, IndexSpec};
use crate::processing_element::program::validate::ValidatedRegion;

struct MatrixRoles {
    m: String,
    n: String,
    k: String,
    batch: Vec<String>,
}
impl MatrixRoles {
    fn shape(
        &self,
        region: &ValidatedRegion,
        intervals: &BTreeMap<String, Range<usize>>,
    ) -> Result<MatrixWorkShape, SimError> {
        let length = |axis: &str| -> Result<usize, SimError> {
            let full = region
                .extent(axis)
                .ok_or_else(|| SimError("unknown matrix axis".into()))?;
            let range = intervals.get(axis).cloned().unwrap_or(0..full);
            if range.start >= range.end || range.end > full {
                return Err(SimError(
                    "matrix interval is outside resolved extent".into(),
                ));
            }
            Ok(range.len())
        };
        for axis in &self.batch {
            if length(axis)? != 1 {
                return Err(SimError(
                    "matrix service requires singleton resolved batch intervals".into(),
                ));
            }
        }
        Ok(MatrixWorkShape {
            m: length(&self.m)?,
            n: length(&self.n)?,
            k: length(&self.k)?,
        })
    }
}
#[expect(
    clippy::too_many_lines,
    reason = "Derive the matrix signature from one validated assignment"
)]
pub(super) fn work(
    model: &ComputeModel,
    assignment: &AssignmentSpec,
    region: &ValidatedRegion,
    intervals: &BTreeMap<String, Range<usize>>,
) -> Result<Option<ComputeWork>, SimError> {
    let ExprSpec::MatrixMac {
        args,
        over,
        execution,
        ..
    } = &assignment.expr
    else {
        return Ok(None);
    };
    let inputs = args
        .iter()
        .map(|arg| {
            let ExprSpec::Read { tensor, .. } = arg else {
                return Err(SimError("matrix operand must be a direct read".into()));
            };
            region
                .value(tensor)
                .map(|value| value.dtype)
                .ok_or_else(|| SimError(format!("unknown matrix operand {tensor}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let plain =
        ComputeWork::operation("matrix_mac", inputs.clone(), 1).with_execution(execution.clone());
    if !model.requires_matrix_shape(&plain.stages[0].operations[0].signature) {
        return Ok(None);
    }
    let error = || {
        SimError(
            "matrix service requires in-range two-free-axis/one-contraction geometry with singleton shared batch intervals"
                .into(),
        )
    };
    if args.len() != 2
        || over.len() != 1
        || assignment.axes.len() < 2
        || assignment
            .axes
            .iter()
            .enumerate()
            .any(|(i, axis)| assignment.axes[..i].contains(axis))
        || assignment.axes.contains(&over[0])
    {
        return Err(error());
    }
    let contraction = &over[0];
    let mut operands = Vec::new();
    for arg in args {
        let ExprSpec::Read {
            tensor,
            indices,
            bounds,
        } = arg
        else {
            return Err(error());
        };
        let shape = &region.value(tensor).ok_or_else(error)?.shape;
        if *bounds != BoundsMode::RequireInRange || indices.len() != shape.len() || shape.len() < 2
        {
            return Err(error());
        }
        let mut axes = Vec::new();
        for (index, size) in indices.iter().zip(shape) {
            let IndexSpec::Axis(axis) = index else {
                return Err(error());
            };
            if region.extent(axis) != Some(*size) || axes.contains(axis) {
                return Err(error());
            }
            axes.push(axis.clone());
        }
        if !axes.contains(contraction) {
            return Err(error());
        }
        operands.push(axes);
    }
    let batch: Vec<_> = operands[0]
        .iter()
        .filter(|axis| *axis != contraction && operands[1].contains(axis))
        .cloned()
        .collect();
    let free: Vec<Vec<_>> = operands
        .iter()
        .enumerate()
        .map(|(index, axes)| {
            axes.iter()
                .filter(|axis| *axis != contraction && !operands[1 - index].contains(axis))
                .cloned()
                .collect()
        })
        .collect();
    if free.iter().any(|axes| axes.len() != 1)
        || assignment.axes.len() != batch.len() + 2
        || batch
            .iter()
            .chain(free.iter().flatten())
            .any(|axis| !assignment.axes.contains(axis))
    {
        return Err(error());
    }
    let output = &region
        .value(&assignment.primary_result())
        .ok_or_else(error)?
        .shape;
    if output.len() != assignment.axes.len()
        || output
            .iter()
            .zip(&assignment.axes)
            .any(|(size, axis)| region.extent(axis) != Some(*size))
    {
        return Err(error());
    }
    let roles = MatrixRoles {
        m: free[0][0].clone(),
        n: free[1][0].clone(),
        k: contraction.clone(),
        batch,
    };
    Ok(Some(
        ComputeWork::matrix_mac(inputs, roles.shape(region, intervals)?)?
            .with_execution(execution.clone()),
    ))
}
