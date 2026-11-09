// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Numerical side effects at the ordinary runner's completed assignment
//! boundary.

use std::collections::BTreeMap;
use std::ops::Range;

use gwr_engine::types::{SimError, SimResult};

use super::compute::Sparsity;
use super::program::syntax::{BoundsMode, ExprSpec, IndexSpec, ReductionOp};
use super::program::tile::RegionTile;
use super::program::validate::ValidatedRegion;
use super::task::ProgramTaskConfig;
use super::tensors::dtype::DataType;
use super::tensors::{HasShape, TensorView};
use crate::memory::functional_value::{apply, floating, signed, unsigned, validate_operation};
use crate::memory::{FunctionalContext, FunctionalValue};

/// Validate numerical domains before any program traffic or side effects.
pub(super) fn preflight(config: &ProgramTaskConfig) -> SimResult {
    if config.pipeline.is_some() || config.program.local_round_policy.is_some() {
        return Err(SimError(
            "functional mode does not support pipelines or local metadata policies".into(),
        ));
    }
    let region = super::program::validate::validate_program(
        &config.program,
        &super::program::validate::RegionBindings {
            inputs: config.inputs.clone(),
            outputs: config.outputs.clone(),
        },
    )?;
    for assignment in &config.program.assignments {
        if assignment.indices_output.is_some() {
            return Err(SimError(
                "functional mode does not support index outputs".into(),
            ));
        }
        let dtype = region.value(&assignment.primary_result()).unwrap().dtype;
        let actual = validate_expr(&assignment.expr, &region, dtype)?;
        if actual != dtype {
            return Err(SimError("functional read assignment requires explicit convert or reinterpret for dtype changes".into()));
        }
    }
    Ok(())
}

fn validate_expr(
    expr: &ExprSpec,
    region: &ValidatedRegion,
    result: DataType,
) -> Result<DataType, SimError> {
    match expr {
        ExprSpec::Read { tensor, .. } => Ok(region.value(tensor).unwrap().dtype),
        ExprSpec::Call { op, args, attrs } => {
            if !attrs.is_empty() {
                return Err(SimError(format!(
                    "unsupported functional attributes for '{op}'"
                )));
            }
            let inputs = args
                .iter()
                .map(|arg| validate_expr(arg, region, result))
                .collect::<Result<Vec<_>, _>>()?;
            validate_operation(op, &inputs, result)?;
            Ok(result)
        }
        ExprSpec::MatrixMac {
            execution,
            args,
            quantization,
            ..
        } => {
            if args.len() != 2
                || execution
                    .as_ref()
                    .is_some_and(|mode| mode.sparsity != Sparsity::Dense)
            {
                return Err(SimError(
                    "functional matrix_mac requires two dense operands".into(),
                ));
            }
            let accumulator = execution.as_ref().map_or(result, |mode| mode.accumulator);
            if accumulator == DataType::Bool {
                return Err(SimError(
                    "functional matrix_mac cannot accumulate Bool".into(),
                ));
            }
            if quantization.is_none() {
                let inputs = args
                    .iter()
                    .map(|arg| validate_expr(arg, region, result))
                    .collect::<Result<Vec<_>, _>>()?;
                validate_operation("mul", &inputs, accumulator)?;
            }
            if !((floating(accumulator) && floating(result))
                || (signed(accumulator) && signed(result))
                || (unsigned(accumulator) && unsigned(result)))
            {
                return Err(SimError(
                    "functional matrix result requires compatible numeric accumulator domain"
                        .into(),
                ));
            }
            Ok(result)
        }
        ExprSpec::Reduce { op, value, .. } => {
            let input = validate_expr(value, region, result)?;
            let operation = match op {
                ReductionOp::Sum => "add",
                ReductionOp::Max => "maximum",
                ReductionOp::Min => "minimum",
            };
            let comparison = if *op == ReductionOp::Sum {
                result
            } else {
                input
            };
            validate_operation(operation, &[comparison, input], comparison)?;
            Ok(result)
        }
    }
}

pub(super) fn evaluate_assignment(
    context: &FunctionalContext,
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    tile: &RegionTile,
    assignment_index: usize,
) -> SimResult {
    let assignment = &config.program.assignments[assignment_index];
    let dtype = region.value(&assignment.primary_result()).unwrap().dtype;
    let fp32 = dtype == DataType::Fp32
        && config.inputs.iter().all(|v| *v.dtype() == DataType::Fp32)
        && config
            .program
            .temporaries
            .values()
            .all(|v| v.dtype == DataType::Fp32)
        && !matches!(
            &assignment.expr,
            ExprSpec::MatrixMac {
                quantization: Some(_),
                ..
            }
        )
        && !matches!(&assignment.expr, ExprSpec::MatrixMac { execution: Some(mode), .. } if mode.accumulator != DataType::Fp32);
    if fp32 && super::functional_gemm::evaluate(context, config, region, tile, assignment_index)? {
        return Ok(());
    }
    if fp32
        && super::functional_convolution::evaluate(context, config, region, tile, assignment_index)?
    {
        return Ok(());
    }
    let assignment = &config.program.assignments[assignment_index];
    let name = assignment.primary_result();
    let ranges = assignment
        .axes
        .iter()
        .map(|axis| {
            tile.output_intervals
                .get(axis)
                .cloned()
                .unwrap_or(0..region.extent(axis).unwrap())
        })
        .collect::<Vec<_>>();
    let mut coordinates = BTreeMap::new();
    let mut results = Vec::new();
    visit(
        &assignment.axes,
        &ranges,
        0,
        &mut coordinates,
        &mut |coordinates| {
            let value = eval(
                context,
                config,
                region,
                &assignment.expr,
                coordinates,
                dtype,
            )?
            .ok_or_else(|| SimError("excluded value outside reduction".into()))?;
            let index = assignment
                .axes
                .iter()
                .map(|axis| coordinates[axis])
                .collect::<Vec<_>>();
            results.push((index, value));
            Ok(())
        },
    )?;
    // Snapshot all reads before publishing this assignment's output values.
    // Physical writes still occur only when the actual store reaches memory.
    for (index, value) in results {
        if let Some(output) = config
            .program
            .outputs
            .iter()
            .position(|output| output == &name)
        {
            let address = element_bit_address(&config.outputs[output], &index)?;
            context.write_value(address, value)?;
        }
        if config.program.assignments.len() > 1 {
            context
                .values
                .borrow_mut()
                .insert((name.clone(), index), value);
        }
    }
    Ok(())
}

fn eval(
    context: &FunctionalContext,
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    expr: &ExprSpec,
    coordinates: &BTreeMap<String, usize>,
    dtype: DataType,
) -> Result<Option<FunctionalValue>, SimError> {
    match expr {
        ExprSpec::Read {
            tensor,
            indices,
            bounds,
        } => eval_read(
            context,
            config,
            region,
            tensor,
            indices,
            *bounds,
            coordinates,
        ),
        ExprSpec::Call { op, args, .. } => {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                let Some(value) = eval(context, config, region, arg, coordinates, dtype)? else {
                    return Ok(None);
                };
                values.push(value);
            }
            Ok(Some(apply(op, &values, dtype)?))
        }

        ExprSpec::MatrixMac { .. } => {
            eval_matrix(context, config, region, expr, coordinates, dtype)
        }
        ExprSpec::Reduce { op, over, value } => {
            let ranges = over
                .iter()
                .map(|axis| 0..region.extent(axis).unwrap())
                .collect::<Vec<_>>();
            let mut coordinates = coordinates.clone();
            let mut accumulated = if *op == ReductionOp::Sum {
                Some(FunctionalValue::from_bits(dtype, 0)?)
            } else {
                None
            };
            visit(over, &ranges, 0, &mut coordinates, &mut |coordinates| {
                if let Some(value) = eval(context, config, region, value, coordinates, dtype)? {
                    let value = if *op == ReductionOp::Sum {
                        value.convert(dtype)?
                    } else {
                        value
                    };
                    accumulated = Some(match accumulated {
                        None => value,
                        Some(previous) => apply(
                            match op {
                                ReductionOp::Sum => "add",
                                ReductionOp::Max => "maximum",
                                ReductionOp::Min => "minimum",
                            },
                            &[previous, value],
                            value.dtype(),
                        )?,
                    });
                }
                Ok(())
            })?;
            Ok(Some(match accumulated {
                Some(value) => value.convert(dtype)?,
                None if *op == ReductionOp::Sum => FunctionalValue::from_bits(dtype, 0)?,
                None => {
                    return Err(SimError(
                        "functional extremum reduction has no included values".into(),
                    ));
                }
            }))
        }
    }
}

fn eval_matrix(
    context: &FunctionalContext,
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    expr: &ExprSpec,
    coordinates: &BTreeMap<String, usize>,
    dtype: DataType,
) -> Result<Option<FunctionalValue>, SimError> {
    let ExprSpec::MatrixMac {
        args,
        over,
        execution,
        quantization,
    } = expr
    else {
        unreachable!("matrix expression dispatched")
    };
    let accumulator = execution.as_ref().map_or(dtype, |mode| mode.accumulator);
    let ranges = over
        .iter()
        .map(|axis| 0..region.extent(axis).unwrap())
        .collect::<Vec<_>>();
    let mut coordinates = coordinates.clone();
    let mut sum = FunctionalValue::from_bits(accumulator, 0)?;
    visit(over, &ranges, 0, &mut coordinates, &mut |coordinates| {
        let left = eval(context, config, region, &args[0], coordinates, dtype)?;
        let right = eval(context, config, region, &args[1], coordinates, dtype)?;
        if let (Some(mut left), Some(mut right)) = (left, right) {
            if let Some([left_operand, right_operand]) = quantization {
                left = scaled_value(
                    context,
                    config,
                    region,
                    &args[0],
                    left_operand,
                    coordinates,
                    left,
                )?;
                right = scaled_value(
                    context,
                    config,
                    region,
                    &args[1],
                    right_operand,
                    coordinates,
                    right,
                )?;
            }
            // Retain operand precision until the product is rounded to
            // the declared accumulator, then round the separate
            // addition.
            let product = apply("mul", &[left, right], accumulator)?;
            sum = apply("add", &[sum, product], accumulator)?;
        }
        Ok(())
    })?;
    Ok(Some(sum.convert(dtype)?))
}

fn eval_read(
    context: &FunctionalContext,
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    tensor: &str,
    indices: &[IndexSpec],
    bounds: BoundsMode,
    coordinates: &BTreeMap<String, usize>,
) -> Result<Option<FunctionalValue>, SimError> {
    let shape = &region
        .value(tensor)
        .ok_or_else(|| SimError(format!("unknown functional tensor {tensor}")))?
        .shape;
    let mut index = Vec::with_capacity(indices.len());
    for (spec, &extent) in indices.iter().zip(shape) {
        let coordinate = match spec {
            IndexSpec::Axis(axis) => {
                if extent == 1 {
                    0
                } else {
                    *coordinates
                        .get(axis)
                        .ok_or_else(|| SimError(format!("unknown functional axis {axis}")))?
                        as i128
                }
            }
            IndexSpec::Zero(value) => *value as i128,
            IndexSpec::Affine { affine } => affine.terms.iter().try_fold(
                i128::from(affine.offset),
                |sum, (axis, coefficient)| {
                    coordinates
                        .get(axis)
                        .map(|value| sum + (*value as i128) * (*coefficient as i128))
                        .ok_or_else(|| SimError(format!("unknown functional axis {axis}")))
                },
            )?,
        };
        if coordinate < 0 || coordinate >= extent as i128 {
            if bounds == BoundsMode::Exclude {
                return Ok(None);
            }
            return Err(SimError("functional tensor index is out of bounds".into()));
        }
        index.push(coordinate as usize);
    }
    if let Some(input) = config
        .program
        .inputs
        .iter()
        .position(|input| input == tensor)
    {
        let view = &config.inputs[input];
        let address = element_bit_address(view, &index)?;
        return Ok(Some(context.local.read_value(address, *view.dtype())?));
    }
    context
        .values
        .borrow()
        .get(&(tensor.to_owned(), index))
        .copied()
        .map(Some)
        .ok_or_else(|| {
            SimError(format!(
                "functional value '{tensor}' is unavailable before its producer"
            ))
        })
}

fn visit(
    axes: &[String],
    ranges: &[Range<usize>],
    depth: usize,
    coordinates: &mut BTreeMap<String, usize>,
    callback: &mut impl FnMut(&BTreeMap<String, usize>) -> SimResult,
) -> SimResult {
    if depth == axes.len() {
        return callback(coordinates);
    }
    for coordinate in ranges[depth].clone() {
        coordinates.insert(axes[depth].clone(), coordinate);
        visit(axes, ranges, depth + 1, coordinates, callback)?;
    }
    coordinates.remove(&axes[depth]);
    Ok(())
}

pub(super) fn element_bit_address(view: &TensorView, index: &[usize]) -> Result<u128, SimError> {
    if index.len() != view.num_dims() {
        return Err(SimError("functional tensor rank mismatch".into()));
    }
    let mut linear = 0usize;
    for (((&coordinate, &extent), &offset), &tensor_extent) in index
        .iter()
        .zip(view.shape().dims())
        .zip(view.offsets())
        .zip(view.tensor().shape().dims())
    {
        if coordinate >= extent {
            return Err(SimError("functional view coordinate out of bounds".into()));
        }
        linear = linear
            .checked_mul(tensor_extent)
            .and_then(|value| value.checked_add(coordinate + offset))
            .ok_or_else(|| SimError("functional tensor index overflows".into()))?;
    }
    (view.tensor().addr() as u128 * 8)
        .checked_add((linear as u128) * view.dtype().num_bits() as u128)
        .filter(|address| *address + view.dtype().num_bits() as u128 <= (u64::MAX as u128 + 1) * 8)
        .ok_or_else(|| SimError("functional tensor address overflows".into()))
}

pub(super) fn element_address(view: &TensorView, index: &[usize]) -> Result<u64, SimError> {
    let address = element_bit_address(view, index)?;
    if address % 8 != 0 {
        return Err(SimError(
            "functional tensor element is not byte aligned".into(),
        ));
    }
    Ok((address / 8) as u64)
}

fn scaled_value(
    context: &FunctionalContext,
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    expr: &ExprSpec,
    operand: &super::program::syntax::ScaledOperandSpec,
    coordinates: &BTreeMap<String, usize>,
    value: FunctionalValue,
) -> Result<FunctionalValue, SimError> {
    use super::program::syntax::PackedMatrixFormat;
    let ExprSpec::Read {
        tensor, indices, ..
    } = expr
    else {
        unreachable!("validated packed operand")
    };
    let shape = &region.value(tensor).unwrap().shape;
    let mut index = indices
        .iter()
        .zip(shape)
        .map(|(index, &extent)| match index {
            IndexSpec::Axis(axis) => {
                if extent == 1 {
                    0
                } else {
                    coordinates[axis]
                }
            }
            IndexSpec::Zero(value) => *value,
            IndexSpec::Affine { .. } => unreachable!("validated packed indices"),
        })
        .collect::<Vec<_>>();
    *index.last_mut().unwrap() /= operand.block_size;
    let scale_index = config
        .program
        .inputs
        .iter()
        .position(|name| name == &operand.block_scales)
        .unwrap();
    let view = &config.inputs[scale_index];
    let bits = context
        .local
        .read_value(element_bit_address(view, &index)?, DataType::Fp8)?
        .bits() as u8;
    let scale = match operand.format {
        PackedMatrixFormat::Nvfp4 => crate::memory::functional_value::decode_e4m3fn(bits),
        PackedMatrixFormat::Mxfp4 => crate::memory::functional_value::decode_e8m0(bits),
    };
    let global = if let Some(name) = &operand.global_scale {
        let index = config
            .program
            .inputs
            .iter()
            .position(|input| input == name)
            .unwrap();
        let view = &config.inputs[index];
        context
            .local
            .read_value(element_bit_address(view, &[0])?, DataType::Fp32)?
            .as_f32()?
    } else {
        1.
    };
    FunctionalValue::from_f32(DataType::Fp32, value.as_f32()? * scale * global)
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::super::program::tile::plan_tiles;
    use super::super::program::validate::{RegionBindings, validate_program};
    use super::super::tensors::Tensor;
    use super::*;

    fn config(
        program: &str,
        inputs: &[(DataType, &[usize])],
        output: DataType,
        shape: &[usize],
    ) -> ProgramTaskConfig {
        let view = |dtype, shape: &[usize], address| {
            TensorView::new_full(Tensor::new(shape, &dtype, address).unwrap())
        };
        ProgramTaskConfig {
            id: "typed".into(),
            program: yaml_serde::from_str(program).unwrap(),
            tiling: None,
            pipeline: None,
            inputs: inputs
                .iter()
                .enumerate()
                .map(|(i, (dtype, shape))| view(*dtype, shape, (i * 4096) as u64))
                .collect(),
            outputs: vec![view(output, shape, 16384)],
        }
    }

    fn evaluate(
        config: &ProgramTaskConfig,
        values: &[Vec<FunctionalValue>],
    ) -> Vec<FunctionalValue> {
        preflight(config).unwrap();
        let region = validate_program(
            &config.program,
            &RegionBindings {
                inputs: config.inputs.clone(),
                outputs: config.outputs.clone(),
            },
        )
        .unwrap();
        let tile = plan_tiles(&region, None).unwrap().tiles().next().unwrap();
        let context = FunctionalContext::new(Rc::default());
        for (view, values) in config.inputs.iter().zip(values) {
            context
                .local
                .write_values(view.tensor().addr(), *view.dtype(), values)
                .unwrap();
        }
        evaluate_assignment(&context, config, &region, &tile, 0).unwrap();
        context
            .local
            .read_values(
                config.outputs[0].tensor().addr(),
                *config.outputs[0].dtype(),
                config.outputs[0].num_elements(),
            )
            .unwrap()
    }

    #[test]
    fn undefined_domains_are_rejected_by_preflight() {
        for (dtype, expr) in [
            (
                DataType::Bool,
                "{add: [{read: {tensor: A, indices: [i]}}, {read: {tensor: A, indices: [i]}}]}",
            ),
            (
                DataType::Int64,
                "{sqrt: [{read: {tensor: A, indices: [i]}}]}",
            ),
        ] {
            let program = format!(
                "inputs: [A]\noutputs: [Y]\nassignments: [{{output: Y, axes: [i], expr: {expr}}}]"
            );
            let config = config(&program, &[(dtype, &[2])], dtype, &[2]);
            assert!(
                preflight(&config)
                    .unwrap_err()
                    .to_string()
                    .contains("undefined functional")
            );
        }
        let config = config(
            "inputs: [A, B]\noutputs: [Y]\nassignments: [{output: Y, axes: [m,n], expr: {matrix_mac: {over: [k], execution: {instruction: tensor, accumulator: int32}, args: [{read: {tensor: A, indices: [m,k]}}, {read: {tensor: B, indices: [k,n]}}]}}}]",
            &[(DataType::Int8, &[1, 1]), (DataType::Int8, &[1, 1])],
            DataType::Bool,
            &[1, 1],
        );
        assert!(
            preflight(&config)
                .unwrap_err()
                .to_string()
                .contains("numeric accumulator domain")
        );
    }

    #[test]
    fn extremum_compares_in_the_source_domain_before_final_conversion() {
        let config = config(
            "inputs: [A]\noutputs: [Y]\nextents: {i: 1}\nassignments: [{output: Y, axes: [i], expr: {reduce: {op: min, over: [k], value: {read: {tensor: A, indices: [k]}}}}}]",
            &[(DataType::Int64, &[2])],
            DataType::Int8,
            &[1],
        );
        let result = evaluate(
            &config,
            &[vec![
                FunctionalValue::from_i64(DataType::Int64, 256).unwrap(),
                FunctionalValue::from_i64(DataType::Int64, 127).unwrap(),
            ]],
        );
        assert_eq!(result[0].as_i64().unwrap(), 127);
    }

    #[test]
    fn matrix_accumulator_rounds_products_without_narrowing_operands_first() {
        let config = config(
            "inputs: [A, B]\noutputs: [Y]\nassignments: [{output: Y, axes: [m,n], expr: {matrix_mac: {over: [k], execution: {instruction: tensor, accumulator: bf16}, args: [{read: {tensor: A, indices: [m,k]}}, {read: {tensor: B, indices: [k,n]}}]}}}]",
            &[(DataType::Fp32, &[1, 1]), (DataType::Fp32, &[1, 1])],
            DataType::Fp32,
            &[1, 1],
        );
        let f = |x| FunctionalValue::from_f32(DataType::Fp32, x).unwrap();
        let result = evaluate(&config, &[vec![f(1.003)], vec![f(1.003)]]);
        assert_eq!(result[0].as_f32().unwrap(), 1.007_812_5);
    }

    #[test]
    fn integer_matrix_accumulators_keep_all_64_bits() {
        for dtype in DataType::ALL
            .into_iter()
            .filter(|dtype| signed(*dtype) || unsigned(*dtype))
        {
            let accumulator = if signed(dtype) {
                DataType::Int64
            } else {
                DataType::Uint64
            };
            let program = format!(
                "inputs: [A, B]\noutputs: [Y]\nassignments: [{{output: Y, axes: [m,n], expr: {{matrix_mac: {{over: [k], execution: {{instruction: tensor, accumulator: {}}}, args: [{{read: {{tensor: A, indices: [m,k]}}}}, {{read: {{tensor: B, indices: [k,n]}}}}]}}}}}}]",
                format!("{accumulator:?}").to_lowercase()
            );
            let config = config(
                &program,
                &[(dtype, &[1, 2]), (dtype, &[2, 1])],
                accumulator,
                &[1, 1],
            );
            let v = |x| {
                if signed(dtype) {
                    FunctionalValue::from_i64(dtype, x).unwrap()
                } else {
                    FunctionalValue::from_u64(dtype, x as u64).unwrap()
                }
            };
            let x = if dtype.num_bits() == 64 {
                9_007_199_254_740_993
            } else {
                3
            };
            let result = evaluate(&config, &[vec![v(x), v(1)], vec![v(2), v(1)]]);
            assert_eq!(result[0].bits(), (x * 2 + 1) as u64);
        }
    }
}
