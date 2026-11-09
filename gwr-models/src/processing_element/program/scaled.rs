// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Packed matrix operand validation. Scale planes are real metadata inputs;
//! the runner loads full physical views before arithmetic in each output tile.

use std::collections::BTreeMap;

use gwr_engine::types::SimError;

use super::syntax::{ExprSpec, IndexSpec, PackedMatrixFormat, ProgramSpec, ScaleLayout};
use super::validate::TensorSpec;
use crate::processing_element::compute::Sparsity;
use crate::processing_element::tensors::dtype::DataType;

pub(super) fn validate_scaled_matrix(
    expr: &ExprSpec,
    spec: &ProgramSpec,
    values: &BTreeMap<String, TensorSpec>,
) -> Result<(), SimError> {
    let ExprSpec::MatrixMac {
        execution,
        args,
        over,
        quantization,
    } = expr
    else {
        return Ok(());
    };
    let native_tag = execution
        .as_ref()
        .is_some_and(|mode| matches!(mode.instruction.as_str(), "nvfp4" | "mxfp4"));
    let Some(operands) = quantization else {
        return if native_tag {
            Err(SimError(
                "native scaled matrix instruction requires quantization metadata".into(),
            ))
        } else {
            Ok(())
        };
    };
    let format = operands[0].format;
    if operands[1].format != format
        || execution.as_ref().is_none_or(|mode| {
            mode.instruction != format.instruction()
                || mode.accumulator != DataType::Fp32
                || mode.sparsity != Sparsity::Dense
        })
    {
        return Err(SimError("scaled matrix requires a matching format pair and explicit dense native instruction with FP32 accumulation".into()));
    }
    for (arg, operand) in args.iter().zip(operands) {
        let ExprSpec::Read {
            tensor, indices, ..
        } = arg
        else {
            unreachable!("executable expression validated first")
        };
        let value = values
            .get(tensor)
            .ok_or_else(|| SimError(format!("missing packed input {tensor}")))?;
        if !spec.inputs.contains(tensor)
            || value.dtype != DataType::Fp4
            || operand.block_size != format.block_size()
        {
            return Err(SimError(
                "scaled matrix requires packed Fp4 boundary inputs and exact format block size"
                    .into(),
            ));
        }
        if indices.last() != Some(&IndexSpec::Axis(over[0].clone())) {
            return Err(SimError(
                "scaled matrix reduction axis must be final operand storage dimension".into(),
            ));
        }
        let scales = metadata_value(&operand.block_scales, spec, values)?;
        if scales.dtype != DataType::Fp8 {
            return Err(SimError("block scales require one-byte Fp8 storage; explicit format determines E4M3 or E8M0 encoding".into()));
        }
        let mut shape = value.shape.clone();
        let last = shape
            .last_mut()
            .ok_or_else(|| SimError("packed input must have dimensions".into()))?;
        *last = checked_ceil(*last, operand.block_size)?;
        if operand.layout == ScaleLayout::Padded128x4 {
            if shape.len() != 2 {
                return Err(SimError(
                    "padded128x4 scales require rank-two packed operands".into(),
                ));
            }
            shape[0] = checked_round_up(shape[0], 128)?;
            shape[1] = checked_round_up(shape[1], 4)?;
        }
        shape.iter().try_fold(1usize, |count, dimension| {
            count
                .checked_mul(*dimension)
                .ok_or_else(|| SimError("scale shape product overflows".into()))
        })?;
        if scales.shape != shape {
            return Err(SimError(format!(
                "block scale {} shape {:?} must be {:?}",
                operand.block_scales, scales.shape, shape
            )));
        }
        match (format, &operand.global_scale) {
            (PackedMatrixFormat::Nvfp4, Some(name)) => {
                let global = metadata_value(name, spec, values)?;
                if global.dtype != DataType::Fp32 || global.shape != [1] {
                    return Err(SimError("NVFP4 global scale must be one FP32 value".into()));
                }
            }
            (PackedMatrixFormat::Mxfp4, None) => {}
            _ => {
                return Err(SimError(
                    "NVFP4 requires a global scale; MXFP4 forbids one".into(),
                ));
            }
        }
    }
    Ok(())
}

fn metadata_value<'a>(
    name: &str,
    spec: &ProgramSpec,
    values: &'a BTreeMap<String, TensorSpec>,
) -> Result<&'a TensorSpec, SimError> {
    if !spec.inputs.iter().any(|input| input == name)
        || !spec.metadata_inputs.iter().any(|input| input == name)
    {
        return Err(SimError(format!(
            "scale {name} must be a boundary input listed in metadata_inputs for physical loads"
        )));
    }
    values
        .get(name)
        .ok_or_else(|| SimError(format!("unknown scale tensor {name}")))
}

fn checked_ceil(value: usize, divisor: usize) -> Result<usize, SimError> {
    value
        .checked_add(divisor - 1)
        .map(|value| value / divisor)
        .ok_or_else(|| SimError("scale block ceil division overflows".into()))
}

fn checked_round_up(value: usize, multiple: usize) -> Result<usize, SimError> {
    checked_ceil(value, multiple)?
        .checked_mul(multiple)
        .ok_or_else(|| SimError("physical scale padding overflows".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_tail_and_padding_arithmetic_is_checked() {
        assert_eq!(checked_ceil(33, 16).unwrap(), 3);
        assert_eq!(checked_round_up(3, 4).unwrap(), 4);
        assert!(checked_ceil(usize::MAX, 16).is_err());
        assert_eq!(
            checked_round_up(usize::MAX - 127, 128).unwrap(),
            usize::MAX - 127
        );
        assert!(checked_round_up(usize::MAX - 126, 128).is_err());
    }
}
