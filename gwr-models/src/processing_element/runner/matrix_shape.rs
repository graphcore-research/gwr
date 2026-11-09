// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Canonical unbatched role derivation shared by preflight and actual chunks.
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
        Ok(MatrixWorkShape {
            m: length(&self.m)?,
            n: length(&self.n)?,
            k: length(&self.k)?,
        })
    }
}
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
            "matrix service requires in-range unbatched two-free-axis/one-contraction geometry"
                .into(),
        )
    };
    if args.len() != 2
        || over.len() != 1
        || assignment.axes.len() != 2
        || assignment.axes[0] == assignment.axes[1]
        || assignment.axes.contains(&over[0])
    {
        return Err(error());
    }
    let contraction = &over[0];
    let mut free = Vec::new();
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
        if *bounds != BoundsMode::RequireInRange || indices.len() != 2 || shape.len() != 2 {
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
        let role = axes
            .iter()
            .find(|axis| *axis != contraction)
            .ok_or_else(error)?;
        if !assignment.axes.contains(role) {
            return Err(error());
        }
        free.push(role.clone());
    }
    if free[0] == free[1] {
        return Err(error());
    }
    let output = &region
        .value(&assignment.primary_result())
        .ok_or_else(error)?
        .shape;
    if output.len() != 2
        || output
            .iter()
            .zip(&assignment.axes)
            .any(|(size, axis)| region.extent(axis) != Some(*size))
    {
        return Err(error());
    }
    let roles = MatrixRoles {
        m: free[0].clone(),
        n: free[1].clone(),
        k: contraction.clone(),
    };
    Ok(Some(
        ComputeWork::matrix_mac(inputs, roles.shape(region, intervals)?)?
            .with_execution(execution.clone()),
    ))
}
