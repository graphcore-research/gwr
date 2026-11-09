// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Opt-in checks for caller-declared homogeneous scalar assignments.

use std::collections::BTreeSet;

use gwr_engine::types::SimError;

use super::syntax::ExprSpec;
use super::validate::ValidatedRegion;
use crate::processing_element::tensors::dtype::DataType;

/// Check explicit dtype transitions in the selected assignment outputs.
///
/// The caller asserts that ordinary scalar calls in these assignments require
/// homogeneous operand/result dtypes. Every call result, including a nested
/// child of a cast, uses the enclosing assignment's destination dtype. Unary
/// `convert` explicitly permits a dtype/width transition; unary `reinterpret`
/// permits a dtype transition only at equal logical bit width.
///
/// This pure, opt-in check does not run during ordinary validation or
/// execution. It neither infers promotion nor certifies named operations'
/// numerical semantics. Mixed-signature operations such as comparisons/selects
/// must be left unselected. Selected matrix/reduction expressions are
/// unsupported. Selection must be nonempty and unique, and each output must
/// have exactly one writer. Diagnostics reject unknown reads even if a
/// validated region has subsequently been mutated through its public fields.
pub fn validate_homogeneous_scalar_assignments(
    region: &ValidatedRegion,
    outputs: &[&str],
) -> Result<(), SimError> {
    if outputs.is_empty() {
        return Err(SimError("scalar dtype selection must be nonempty".into()));
    }
    let mut selected = BTreeSet::new();
    for &output in outputs {
        if !selected.insert(output) {
            return Err(SimError(format!(
                "duplicate scalar dtype selection: {output}"
            )));
        }
        let mut writers = region.program.assignments.iter().filter(|assignment| {
            assignment.output.as_deref() == Some(output)
                || assignment.indices_output.as_deref() == Some(output)
        });
        let assignment = writers
            .next()
            .ok_or_else(|| SimError(format!("scalar dtype selection {output} has no writer")))?;
        if writers.next().is_some() {
            return Err(SimError(format!(
                "scalar dtype selection {output} has multiple writers"
            )));
        }
        let dtype = read_dtype(region, output)?;
        let actual = scalar_dtype(&assignment.expr, region, dtype)
            .map_err(|error| SimError(format!("assignment {output}: {}", error.0)))?;
        if actual != dtype {
            return Err(SimError(format!(
                "assignment {output}: read dtype {actual:?} differs from destination {dtype:?}; use explicit convert or reinterpret"
            )));
        }
    }
    Ok(())
}

fn scalar_dtype(
    expr: &ExprSpec,
    region: &ValidatedRegion,
    result: DataType,
) -> Result<DataType, SimError> {
    match expr {
        ExprSpec::Read { tensor, .. } => read_dtype(region, tensor),
        ExprSpec::Call { op, args, .. } => {
            let cast = matches!(op.as_str(), "convert" | "reinterpret");
            if cast && args.len() != 1 {
                return Err(SimError(format!(
                    "{op} requires one operand in homogeneous scalar guard"
                )));
            }
            for arg in args {
                let input = scalar_dtype(arg, region, result)?;
                if op == "reinterpret" && input.num_bits() != result.num_bits() {
                    return Err(SimError(format!(
                        "reinterpret operand {input:?} and destination {result:?} differ in logical bit width"
                    )));
                }
                if !cast && input != result {
                    return Err(SimError(format!(
                        "{op} operand dtype {input:?} differs from destination {result:?}; materialize a typed temporary or use an explicit cast"
                    )));
                }
            }
            Ok(result)
        }
        _ => Err(SimError(
            "matrix/reduction expressions are unsupported by homogeneous scalar guard".into(),
        )),
    }
}

fn read_dtype(region: &ValidatedRegion, tensor: &str) -> Result<DataType, SimError> {
    region
        .value(tensor)
        .map(|value| value.dtype)
        .ok_or_else(|| SimError(format!("unknown scalar dtype tensor {tensor}")))
}
