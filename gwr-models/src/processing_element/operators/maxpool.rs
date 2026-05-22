// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! The MaxPool operator
//!
//! See <https://onnx.ai/onnx/operators/onnx__MaxPool.html#l-onnx-doc-maxpool>

use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use rand::Rng;
use serde::{Deserialize, Deserializer, Serialize};

use super::{Operator, Shape, Tensor, TensorPartition};
use crate::processing_element::ComputeCapabilities;
use crate::processing_element::operators::dtype::DataType;
use crate::processing_element::operators::{
    DimPartition, ExpansionDirection, HasShape, MachineOp, MachineOps, TensorView,
    apply_dim_partitions, partition_across_dimensions,
};

const NAME: &str = "MaxPool";
const BATCH_DIM: usize = 0;
const CHANNEL_DIM: usize = 1;
const FIRST_SPATIAL_DIM: usize = 2;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum AutoPad {
    #[default]
    #[serde(rename = "NOTSET", alias = "notset")]
    NotSet,
    #[serde(rename = "SAME_UPPER", alias = "same_upper")]
    SameUpper,
    #[serde(rename = "SAME_LOWER", alias = "same_lower")]
    SameLower,
    #[serde(rename = "VALID", alias = "valid")]
    Valid,
}

fn deserialize_optional_bool_or_int<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BoolOrInt {
        Bool(bool),
        Int(u8),
    }

    match Option::<BoolOrInt>::deserialize(deserializer)? {
        None => Ok(None),
        Some(BoolOrInt::Bool(value)) => Ok(Some(value)),
        Some(BoolOrInt::Int(0)) => Ok(Some(false)),
        Some(BoolOrInt::Int(1)) => Ok(Some(true)),
        Some(BoolOrInt::Int(value)) => Err(serde::de::Error::custom(format!(
            "expected 0 or 1, got {value}"
        ))),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct OperatorMaxPool {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_pad: Option<AutoPad>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_bool_or_int",
        skip_serializing_if = "Option::is_none"
    )]
    pub ceil_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dilations: Option<Vec<usize>>,
    pub kernel_shape: Vec<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pads: Option<Vec<usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_order: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strides: Option<Vec<usize>>,
}

impl OperatorMaxPool {
    #[must_use]
    pub fn new(kernel_shape: &[usize]) -> Self {
        Self {
            auto_pad: None,
            ceil_mode: None,
            dilations: None,
            kernel_shape: kernel_shape.to_vec(),
            pads: None,
            storage_order: None,
            strides: None,
        }
    }

    fn auto_pad(&self) -> AutoPad {
        self.auto_pad.unwrap_or_default()
    }

    fn ceil_mode(&self) -> bool {
        self.ceil_mode.unwrap_or(false)
    }

    fn normalize_params(&self, spatial_rank: usize) -> Result<PoolParams, SimError> {
        if spatial_rank == 0 {
            return sim_error!("{NAME}: input must contain at least one spatial dimension");
        }

        if self.kernel_shape.len() != spatial_rank {
            return sim_error!(
                "{NAME}: kernel_shape rank {} does not match spatial rank {spatial_rank}",
                self.kernel_shape.len()
            );
        }

        if self.kernel_shape.contains(&0) {
            return sim_error!("{NAME}: kernel_shape entries must be greater than zero");
        }

        let auto_pad = self.auto_pad();
        let storage_order = self.storage_order.unwrap_or(0);
        let pads = self.pads.as_deref().unwrap_or_default();

        if storage_order > 1 {
            return sim_error!("{NAME}: storage_order must be 0 or 1");
        }

        if auto_pad != AutoPad::NotSet && !pads.is_empty() {
            return sim_error!("{NAME}: pads cannot be used with auto_pad");
        }

        let strides = normalize_axis_values(
            "strides",
            self.strides.as_deref().unwrap_or_default(),
            spatial_rank,
            1,
        )?;
        let dilations = normalize_axis_values(
            "dilations",
            self.dilations.as_deref().unwrap_or_default(),
            spatial_rank,
            1,
        )?;
        let pads = if pads.is_empty() {
            vec![0; spatial_rank * 2]
        } else if pads.len() == spatial_rank * 2 {
            pads.to_vec()
        } else {
            return sim_error!(
                "{NAME}: pads rank {} does not match 2 * spatial rank {}",
                pads.len(),
                spatial_rank * 2
            );
        };

        if strides.contains(&0) {
            return sim_error!("{NAME}: strides entries must be greater than zero");
        }
        if dilations.contains(&0) {
            return sim_error!("{NAME}: dilations entries must be greater than zero");
        }

        Ok(PoolParams {
            kernel_shape: self.kernel_shape.clone(),
            strides,
            dilations,
            pads_begin: pads[..spatial_rank].to_vec(),
            pads_end: pads[spatial_rank..].to_vec(),
        })
    }

    fn output_shape_and_resolved_params<T: HasShape>(
        &self,
        input: &T,
    ) -> Result<(Shape, PoolParams), SimError> {
        if input.num_dims() < 3 {
            return sim_error!("{NAME}: input rank must be at least 3");
        }

        let input_dims = input.shape().get_dims();
        let spatial_rank = input.num_dims() - FIRST_SPATIAL_DIM;
        let mut params = self.normalize_params(spatial_rank)?;
        let auto_pad = self.auto_pad();
        let ceil_mode = self.ceil_mode();

        let mut output_dims = vec![input_dims[BATCH_DIM], input_dims[CHANNEL_DIM]];
        let mut resolved_pads_begin = vec![0; spatial_rank];
        let mut resolved_pads_end = vec![0; spatial_rank];

        for axis in 0..spatial_rank {
            let input_dim = input_dims[FIRST_SPATIAL_DIM + axis];
            if input_dim == 0 {
                return sim_error!("{NAME}: input spatial dimensions must be greater than zero");
            }
            let effective_kernel =
                effective_kernel(params.kernel_shape[axis], params.dilations[axis]);
            let stride = params.strides[axis];

            let (output_dim, pad_begin, pad_end) = match auto_pad {
                AutoPad::NotSet => {
                    let pad_begin = params.pads_begin[axis];
                    let pad_end = params.pads_end[axis];
                    let output_dim = explicit_output_dim(
                        input_dim,
                        effective_kernel,
                        stride,
                        pad_begin,
                        pad_end,
                        ceil_mode,
                    )?;
                    (output_dim, pad_begin, pad_end)
                }
                AutoPad::Valid => {
                    let output_dim =
                        valid_output_dim(input_dim, effective_kernel, stride, ceil_mode)?;
                    (output_dim, 0, 0)
                }
                AutoPad::SameUpper | AutoPad::SameLower => {
                    let output_dim = input_dim.div_ceil(stride);
                    let pad_shape =
                        ((output_dim - 1) * stride + effective_kernel).saturating_sub(input_dim);
                    let smaller_side = pad_shape / 2;
                    let larger_side = pad_shape - smaller_side;
                    match auto_pad {
                        AutoPad::SameUpper => (output_dim, smaller_side, larger_side),
                        AutoPad::SameLower => (output_dim, larger_side, smaller_side),
                        AutoPad::NotSet | AutoPad::Valid => unreachable!(),
                    }
                }
            };

            output_dims.push(output_dim);
            resolved_pads_begin[axis] = pad_begin;
            resolved_pads_end[axis] = pad_end;
        }

        params.pads_begin = resolved_pads_begin;
        params.pads_end = resolved_pads_end;

        Ok((Shape(output_dims), params))
    }

    fn infer_input_shape<T: HasShape>(&self, output: &T) -> Result<Shape, SimError> {
        if output.num_dims() < 3 {
            return sim_error!("{NAME}: output rank must be at least 3");
        }

        let output_dims = output.shape().get_dims();
        let spatial_rank = output.num_dims() - FIRST_SPATIAL_DIM;
        let params = self.normalize_params(spatial_rank)?;
        let auto_pad = self.auto_pad();

        let mut input_dims = vec![output_dims[BATCH_DIM], output_dims[CHANNEL_DIM]];
        for axis in 0..spatial_rank {
            let output_dim = output_dims[FIRST_SPATIAL_DIM + axis];
            if output_dim == 0 {
                return sim_error!("{NAME}: output spatial dimensions must be greater than zero");
            }
            let effective_kernel =
                effective_kernel(params.kernel_shape[axis], params.dilations[axis]);
            let stride = params.strides[axis];
            let input_dim = match auto_pad {
                AutoPad::SameUpper | AutoPad::SameLower => (output_dim - 1) * stride + 1,
                AutoPad::NotSet | AutoPad::Valid => ((output_dim - 1) * stride + effective_kernel)
                    .saturating_sub(params.pads_begin[axis] + params.pads_end[axis])
                    .max(1),
            };
            input_dims.push(input_dim);
        }

        Ok(Shape(input_dims))
    }

    fn can_partition_spatial(&self, params: &PoolParams) -> bool {
        self.auto_pad() == AutoPad::NotSet
            && params.pads_begin.iter().all(|pad| *pad == 0)
            && params.pads_end.iter().all(|pad| *pad == 0)
    }
}

impl Default for OperatorMaxPool {
    fn default() -> Self {
        Self::new(&[2, 2])
    }
}

pub fn create_maxpool_op<T: HasShape>(
    tensor: &T,
    direction: ExpansionDirection,
    expand_ratio: f64,
) -> Result<OperatorMaxPool, SimError> {
    if tensor.num_dims() < FIRST_SPATIAL_DIM + 1 {
        return sim_error!(
            "{NAME}: generated MaxPool tensor rank must be at least {}",
            FIRST_SPATIAL_DIM + 1
        );
    }

    let dims = tensor.shape().get_dims();
    let mut kernel_shape = Vec::with_capacity(dims.len() - FIRST_SPATIAL_DIM);
    let mut pads_begin = Vec::with_capacity(dims.len() - FIRST_SPATIAL_DIM);
    let mut pads_end = Vec::with_capacity(dims.len() - FIRST_SPATIAL_DIM);

    for dim in &dims[FIRST_SPATIAL_DIM..] {
        let (kernel, pad_begin, pad_end) = maxpool_axis_params(*dim, expand_ratio, direction);
        kernel_shape.push(kernel);
        pads_begin.push(pad_begin);
        pads_end.push(pad_end);
    }

    let mut operator = OperatorMaxPool::new(&kernel_shape);
    if pads_begin.iter().chain(pads_end.iter()).any(|pad| *pad > 0) {
        let mut pads = pads_begin;
        pads.extend(pads_end);
        operator.pads = Some(pads);
    }

    Ok(operator)
}

#[derive(Clone, Debug)]
struct PoolParams {
    kernel_shape: Vec<usize>,
    strides: Vec<usize>,
    dilations: Vec<usize>,
    pads_begin: Vec<usize>,
    pads_end: Vec<usize>,
}

fn normalize_axis_values(
    name: &str,
    values: &[usize],
    spatial_rank: usize,
    default: usize,
) -> Result<Vec<usize>, SimError> {
    if values.is_empty() {
        Ok(vec![default; spatial_rank])
    } else if values.len() == spatial_rank {
        Ok(values.to_vec())
    } else {
        sim_error!(
            "{NAME}: {name} rank {} does not match spatial rank {spatial_rank}",
            values.len()
        )
    }
}

fn effective_kernel(kernel: usize, dilation: usize) -> usize {
    dilation * (kernel - 1) + 1
}

fn scaled_dim(dim: usize, expand_ratio: f64) -> usize {
    ((dim as f64) * expand_ratio).round().max(1.0) as usize
}

fn split_padding(total_padding: usize) -> (usize, usize) {
    let pad_begin = total_padding / 2;
    let pad_end = total_padding - pad_begin;
    (pad_begin, pad_end)
}

fn maxpool_axis_params(
    dim_size: usize,
    expand_ratio: f64,
    direction: ExpansionDirection,
) -> (usize, usize, usize) {
    let target_dim_size = scaled_dim(dim_size, expand_ratio);
    match direction {
        ExpansionDirection::Forward if target_dim_size <= dim_size => {
            (dim_size - target_dim_size + 1, 0, 0)
        }
        ExpansionDirection::Forward => {
            let (pad_begin, pad_end) = split_padding(target_dim_size - dim_size);
            (1, pad_begin, pad_end)
        }
        ExpansionDirection::Backward if target_dim_size >= dim_size => {
            (target_dim_size - dim_size + 1, 0, 0)
        }
        ExpansionDirection::Backward => {
            let (pad_begin, pad_end) = split_padding(dim_size - target_dim_size);
            (1, pad_begin, pad_end)
        }
    }
}

fn ceil_div_i128(numerator: i128, denominator: i128) -> i128 {
    if numerator >= 0 {
        (numerator + denominator - 1) / denominator
    } else {
        numerator / denominator
    }
}

fn explicit_output_dim(
    input_dim: usize,
    effective_kernel: usize,
    stride: usize,
    pad_begin: usize,
    pad_end: usize,
    ceil_mode: bool,
) -> Result<usize, SimError> {
    let numerator =
        input_dim as i128 + pad_begin as i128 + pad_end as i128 - effective_kernel as i128;
    let stride = stride as i128;
    let mut output_dim = if ceil_mode {
        ceil_div_i128(numerator, stride) + 1
    } else {
        numerator.div_euclid(stride) + 1
    };

    if ceil_mode
        && output_dim > 0
        && (output_dim - 1) * stride >= input_dim as i128 + pad_begin as i128
    {
        output_dim -= 1;
    }

    if output_dim <= 0 {
        return sim_error!("{NAME}: pooling window produces an empty output dimension");
    }
    Ok(output_dim as usize)
}

fn valid_output_dim(
    input_dim: usize,
    effective_kernel: usize,
    stride: usize,
    ceil_mode: bool,
) -> Result<usize, SimError> {
    let output_dim = if ceil_mode {
        ceil_div_i128(
            input_dim as i128 - effective_kernel as i128 + 1,
            stride as i128,
        )
    } else {
        (input_dim as i128 - effective_kernel as i128).div_euclid(stride as i128) + 1
    };

    if output_dim <= 0 {
        return sim_error!("{NAME}: pooling window produces an empty output dimension");
    }
    Ok(output_dim as usize)
}

fn choose_partition_dims<T: HasShape>(output: &T, allow_spatial: bool) -> Vec<usize> {
    output
        .shape()
        .get_dims()
        .iter()
        .enumerate()
        .filter_map(|(dim, size)| {
            (*size > 1 && (allow_spatial || dim < FIRST_SPATIAL_DIM)).then_some(dim)
        })
        .collect()
}

fn validate_inputs<T: HasShape>(inputs: &[Option<T>]) -> Result<&T, SimError> {
    if inputs.len() != 1 {
        return sim_error!("{NAME}: {} inputs found - expected 1", inputs.len());
    }
    inputs[0]
        .as_ref()
        .ok_or(SimError(format!("{NAME}: missing input 0")))
}

fn validate_outputs<T: HasShape>(outputs: &[Option<T>]) -> Result<(&T, Option<&T>), SimError> {
    if !(1..=2).contains(&outputs.len()) {
        return sim_error!("{NAME}: {} outputs found - expected 1 or 2", outputs.len());
    }

    let output = outputs[0]
        .as_ref()
        .ok_or(SimError(format!("{NAME}: missing output 0")))?;
    let indices = outputs.get(1).and_then(Option::as_ref);

    if let Some(indices) = indices
        && indices.shape() != output.shape()
    {
        return sim_error!(
            "{NAME}: Indices shape {:?} must match output shape {:?}",
            indices.shape(),
            output.shape()
        );
    }

    Ok((output, indices))
}

fn validate_input_outputs<'a, 'b, T: HasShape>(
    op: &OperatorMaxPool,
    inputs: &'a [Option<T>],
    outputs: &'b [Option<T>],
) -> Result<(&'a T, &'b T, Option<&'b T>), SimError> {
    let input = validate_inputs(inputs)?;
    let (output, indices) = validate_outputs(outputs)?;

    let (expected_shape, _) = op.output_shape_and_resolved_params(input)?;
    if expected_shape != *output.shape() {
        return sim_error!(
            "{NAME}: Invalid output shape - expected {:?}, found {:?}",
            expected_shape,
            output.shape()
        );
    }

    Ok((input, output, indices))
}

fn validate_tensor_dtypes(input: &Tensor, output: &Tensor, indices: Option<&Tensor>) -> SimResult {
    if input.dtype() != output.dtype() {
        return sim_error!(
            "{NAME}: output dtype {:?} must match input dtype {:?}",
            output.dtype(),
            input.dtype()
        );
    }

    if let Some(indices) = indices
        && *indices.dtype() != DataType::Int64
    {
        return sim_error!(
            "{NAME}: Indices dtype {:?} must be {:?}",
            indices.dtype(),
            DataType::Int64
        );
    }

    Ok(())
}

fn should_add_indices_output(rng: &mut impl Rng, expand_ratio: f64) -> bool {
    if !expand_ratio.is_finite() || expand_ratio <= 0.0 {
        false
    } else if expand_ratio >= 1.0 {
        true
    } else {
        rng.random_bool(expand_ratio)
    }
}

pub fn maybe_add_indices_output(
    outputs: &mut Vec<Option<Tensor>>,
    expand_ratio: f64,
    rng: &mut impl Rng,
) -> Result<bool, SimError> {
    if outputs.len() >= 2 || !should_add_indices_output(rng, expand_ratio) {
        return Ok(false);
    }

    let output = outputs
        .first()
        .and_then(Option::as_ref)
        .ok_or_else(|| SimError(format!("{NAME}: missing output 0")))?;
    outputs.push(Some(Tensor {
        id: None,
        shape: output.shape().clone(),
        dtype: DataType::Int64,
        addr: 0,
    }));
    Ok(true)
}

fn window_valid_element_count(
    output_coordinate: &[usize],
    input_spatial_dims: &[usize],
    params: &PoolParams,
) -> usize {
    output_coordinate
        .iter()
        .enumerate()
        .map(|(axis, output_idx)| {
            let window_start = *output_idx as i128 * params.strides[axis] as i128
                - params.pads_begin[axis] as i128;

            (0..params.kernel_shape[axis])
                .filter(|kernel_idx| {
                    let input_idx = window_start + (*kernel_idx * params.dilations[axis]) as i128;
                    input_idx >= 0 && input_idx < input_spatial_dims[axis] as i128
                })
                .count()
        })
        .product()
}

fn unravel_index(mut linear_idx: usize, dims: &[usize]) -> Vec<usize> {
    let mut coordinate = vec![0; dims.len()];
    for (axis, dim) in dims.iter().enumerate().rev() {
        coordinate[axis] = linear_idx % dim;
        linear_idx /= dim;
    }
    coordinate
}

fn maxpool_comparisons<T: HasShape>(
    op: &OperatorMaxPool,
    inputs: &[Option<T>],
    outputs: &[Option<T>],
) -> Result<usize, SimError> {
    let (input, output, _) = validate_input_outputs(op, inputs, outputs)?;
    let (_, params) = op.output_shape_and_resolved_params(input)?;

    let input_spatial_dims = &input.shape().get_dims()[FIRST_SPATIAL_DIM..];
    let output_spatial_dims = &output.shape().get_dims()[FIRST_SPATIAL_DIM..];
    let num_spatial_outputs = output_spatial_dims.iter().product::<usize>();

    let comparisons_per_batch_channel = (0..num_spatial_outputs)
        .map(|linear_idx| {
            let coordinate = unravel_index(linear_idx, output_spatial_dims);
            window_valid_element_count(&coordinate, input_spatial_dims, &params).saturating_sub(1)
        })
        .sum::<usize>();

    Ok(output.get_dim(output.num_dims(), BATCH_DIM)
        * output.get_dim(output.num_dims(), CHANNEL_DIM)
        * comparisons_per_batch_channel)
}

fn input_partition_for_output_partition(
    input_view: &TensorView,
    partitions: &[DimPartition],
    params: &PoolParams,
) -> Result<TensorView, SimError> {
    let mut input_shape = input_view.shape().get_dims().clone();
    let mut input_offsets = input_view.offsets().get_dims().clone();

    for partition in partitions {
        if input_shape[partition.dim] <= 1 {
            continue;
        }

        if partition.dim < FIRST_SPATIAL_DIM {
            input_offsets[partition.dim] += partition.offset;
            input_shape[partition.dim] = partition.len;
            continue;
        }

        let axis = partition.dim - FIRST_SPATIAL_DIM;
        let effective_kernel = effective_kernel(params.kernel_shape[axis], params.dilations[axis]);
        let first_output = partition.offset;
        let last_output = partition.offset + partition.len - 1;
        let raw_start =
            first_output as i128 * params.strides[axis] as i128 - params.pads_begin[axis] as i128;
        let raw_end = last_output as i128 * params.strides[axis] as i128
            - params.pads_begin[axis] as i128
            + effective_kernel as i128;

        let start = raw_start.clamp(0, input_shape[partition.dim] as i128) as usize;
        let end = raw_end.clamp(0, input_shape[partition.dim] as i128) as usize;
        if start >= end {
            return sim_error!("{NAME}: partition produced an empty input view");
        }

        input_offsets[partition.dim] += start;
        input_shape[partition.dim] = end - start;
    }

    Ok(TensorView::new(
        input_view.tensor().clone(),
        &input_shape,
        &input_offsets,
    ))
}

impl OperatorMaxPool {
    pub fn create_outputs(
        &self,
        inputs: &[Option<Tensor>],
        expand_ratio: f64,
        rng: &mut impl Rng,
    ) -> Result<Vec<Option<Tensor>>, SimError> {
        let input = validate_inputs(inputs)?;
        let (output_shape, _) = self.output_shape_and_resolved_params(input)?;

        let mut outputs = vec![Some(Tensor {
            id: None,
            shape: output_shape,
            dtype: input.dtype,
            addr: 0,
        })];
        maybe_add_indices_output(&mut outputs, expand_ratio, rng)?;
        Ok(outputs)
    }

    pub fn create_inputs(
        &self,
        outputs: &[Option<Tensor>],
        _expand_ratio: f64,
        _rng: &mut impl Rng,
    ) -> Result<Vec<Option<Tensor>>, SimError> {
        let (output, indices) = validate_outputs(outputs)?;
        if let Some(indices) = indices
            && *indices.dtype() != DataType::Int64
        {
            return sim_error!(
                "{NAME}: Indices dtype {:?} must be {:?}",
                indices.dtype(),
                DataType::Int64
            );
        }

        let input_shape = self.infer_input_shape(output)?;
        Ok(vec![Some(Tensor {
            id: None,
            shape: input_shape,
            dtype: output.dtype,
            addr: 0,
        })])
    }
}

impl Operator for OperatorMaxPool {
    fn validate_tensors(&self, inputs: &[Option<Tensor>], outputs: &[Option<Tensor>]) -> SimResult {
        let (input, output, indices) = validate_input_outputs(self, inputs, outputs)?;
        validate_tensor_dtypes(input, output, indices)
    }

    fn compute_delay_ticks(
        &self,
        compute_capabilities: &Rc<ComputeCapabilities>,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) -> Result<usize, SimError> {
        let comparisons = maxpool_comparisons(self, inputs, outputs)?;
        compute_capabilities.cycles_for_ops(comparisons, MachineOp::Compare)
    }

    fn compute_flops(
        &self,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) -> Result<MachineOps, SimError> {
        Ok(MachineOps::from_op(
            MachineOp::Compare,
            maxpool_comparisons(self, inputs, outputs)?,
        ))
    }

    fn partition_views(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
        num_partitions: usize,
    ) -> Result<Vec<TensorPartition>, SimError> {
        let (input_view, output_view, _) = validate_input_outputs(self, input_views, output_views)?;
        let (_, params) = self.output_shape_and_resolved_params(input_view)?;
        let allow_spatial = self.can_partition_spatial(&params);
        let partition_dims = choose_partition_dims(output_view, allow_spatial);
        let output_view_dims = output_view.shape().get_dims();
        let partition_specs =
            partition_across_dimensions(output_view_dims, &partition_dims, num_partitions);

        let mut partitions = Vec::with_capacity(partition_specs.len());
        for spec in partition_specs {
            let (output_shape, partition_offsets) = apply_dim_partitions(output_view_dims, &spec);
            let output_offsets = output_view
                .offsets()
                .get_dims()
                .iter()
                .zip(partition_offsets.iter())
                .map(|(base, offset)| base + offset)
                .collect::<Vec<_>>();

            let input_view = input_partition_for_output_partition(input_view, &spec, &params)?;
            let outputs = output_views
                .iter()
                .map(|maybe_output| {
                    maybe_output.as_ref().map(|view| {
                        TensorView::new(view.tensor().clone(), &output_shape, &output_offsets)
                    })
                })
                .collect::<Vec<_>>();

            partitions.push(TensorPartition {
                inputs: vec![Some(input_view)],
                outputs,
            });
        }

        Ok(partitions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing_element::operators::dtype::DataType;
    use crate::processing_element::operators::{Operator, Tensor, partition_tensors};

    fn tensor(shape: &[usize]) -> Option<Tensor> {
        Some(Tensor::new(shape, &DataType::Bf16, 0))
    }

    fn indices(shape: &[usize]) -> Option<Tensor> {
        Some(Tensor::new(shape, &DataType::Int64, 0))
    }

    fn tensor_view(shape: &[usize]) -> Option<TensorView> {
        let tensor = Tensor::new(shape, &DataType::Bf16, 0);
        Some(TensorView::new_full(tensor))
    }

    fn indices_view(shape: &[usize]) -> Option<TensorView> {
        let tensor = Tensor::new(shape, &DataType::Int64, 0);
        Some(TensorView::new_full(tensor))
    }

    #[test]
    fn new_leaves_optional_attributes_unspecified_and_uses_onnx_defaults() {
        let op = OperatorMaxPool::new(&[2, 2]);
        assert_eq!(op.auto_pad, None);
        assert_eq!(op.ceil_mode, None);
        assert_eq!(op.dilations, None);
        assert_eq!(op.pads, None);
        assert_eq!(op.storage_order, None);
        assert_eq!(op.strides, None);

        let mut rng = rand::rng();
        let outputs = op
            .create_outputs(&[tensor(&[1, 1, 4, 4])], 1.0, &mut rng)
            .unwrap();

        assert_eq!(
            outputs[0].as_ref().unwrap().shape(),
            &Shape::new(&[1, 1, 3, 3])
        );
    }

    #[test]
    fn generated_forward_op_shrinks_with_larger_kernel_when_expand_ratio_is_below_one() {
        let input = Tensor::new(&[1, 1, 10, 8], &DataType::Fp32, 0);
        let op = create_maxpool_op(&input, ExpansionDirection::Forward, 0.5).unwrap();

        assert_eq!(op.kernel_shape, vec![6, 5]);
        assert_eq!(op.pads, None);

        let mut rng = rand::rng();
        let outputs = op.create_outputs(&[Some(input)], 1.0, &mut rng).unwrap();
        assert_eq!(
            outputs[0].as_ref().unwrap().shape(),
            &Shape::new(&[1, 1, 5, 4])
        );
    }

    #[test]
    fn generated_forward_op_grows_with_padding_when_expand_ratio_is_above_one() {
        let input = Tensor::new(&[1, 1, 4, 4], &DataType::Fp32, 0);
        let op = create_maxpool_op(&input, ExpansionDirection::Forward, 1.5).unwrap();

        assert_eq!(op.kernel_shape, vec![1, 1]);
        assert_eq!(op.pads, Some(vec![1, 1, 1, 1]));

        let mut rng = rand::rng();
        let outputs = op.create_outputs(&[Some(input)], 1.0, &mut rng).unwrap();
        assert_eq!(
            outputs[0].as_ref().unwrap().shape(),
            &Shape::new(&[1, 1, 6, 6])
        );
    }

    #[test]
    fn generated_backward_op_uses_expand_ratio_for_input_shape() {
        let output = Tensor::new(&[1, 1, 4, 4], &DataType::Fp32, 0);
        let op = create_maxpool_op(&output, ExpansionDirection::Backward, 0.5).unwrap();

        assert_eq!(op.kernel_shape, vec![1, 1]);
        assert_eq!(op.pads, Some(vec![1, 1, 1, 1]));

        let mut rng = rand::rng();
        let inputs = op.create_inputs(&[Some(output)], 1.0, &mut rng).unwrap();
        assert_eq!(
            inputs[0].as_ref().unwrap().shape(),
            &Shape::new(&[1, 1, 2, 2])
        );
    }

    type OffsetsShapes = (&'static [usize], &'static [usize]);
    type PartitionOffsetsShapes = (OffsetsShapes, OffsetsShapes, OffsetsShapes);

    fn check_partitions(partitions: &[TensorPartition], expected: &[PartitionOffsetsShapes]) {
        assert_eq!(partitions.len(), expected.len());

        for (partition, expected_partition) in partitions.iter().zip(expected.iter()) {
            assert_eq!(partition.inputs.len(), 1);
            assert_eq!(partition.outputs.len(), 2);

            let input = partition.inputs[0].as_ref().unwrap();
            let output = partition.outputs[0].as_ref().unwrap();
            let indices = partition.outputs[1].as_ref().unwrap();

            assert_eq!(
                input.offsets().get_dims().as_slice(),
                expected_partition.0.0
            );
            assert_eq!(input.shape().get_dims().as_slice(), expected_partition.0.1);
            assert_eq!(
                output.offsets().get_dims().as_slice(),
                expected_partition.1.0
            );
            assert_eq!(output.shape().get_dims().as_slice(), expected_partition.1.1);
            assert_eq!(
                indices.offsets().get_dims().as_slice(),
                expected_partition.2.0
            );
            assert_eq!(
                indices.shape().get_dims().as_slice(),
                expected_partition.2.1
            );
        }
    }

    #[test]
    fn create_outputs_returns_y_and_indices() {
        let op = OperatorMaxPool {
            strides: Some(vec![2, 2]),
            ..OperatorMaxPool::new(&[2, 2])
        };
        let mut rng = rand::rng();

        let outputs = op
            .create_outputs(&[tensor(&[1, 3, 4, 4])], 1.0, &mut rng)
            .unwrap();

        assert_eq!(outputs.len(), 2);
        assert_eq!(
            outputs[0].as_ref().unwrap().shape(),
            &Shape::new(&[1, 3, 2, 2])
        );
        assert_eq!(outputs[0].as_ref().unwrap().dtype(), &DataType::Bf16);
        assert_eq!(
            outputs[1].as_ref().unwrap().shape(),
            &Shape::new(&[1, 3, 2, 2])
        );
        assert_eq!(outputs[1].as_ref().unwrap().dtype(), &DataType::Int64);
    }

    #[test]
    fn create_outputs_and_inputs_support_5d_pooling_over_inner_three_dimensions() {
        let op = OperatorMaxPool {
            strides: Some(vec![2, 2, 2]),
            ..OperatorMaxPool::new(&[2, 3, 4])
        };
        let mut rng = rand::rng();

        let outputs = op
            .create_outputs(&[tensor(&[2, 5, 6, 7, 8])], 1.0, &mut rng)
            .unwrap();

        assert_eq!(outputs.len(), 2);
        assert_eq!(
            outputs[0].as_ref().unwrap().shape(),
            &Shape::new(&[2, 5, 3, 3, 3])
        );
        assert_eq!(outputs[0].as_ref().unwrap().dtype(), &DataType::Bf16);
        assert_eq!(
            outputs[1].as_ref().unwrap().shape(),
            &Shape::new(&[2, 5, 3, 3, 3])
        );
        assert_eq!(outputs[1].as_ref().unwrap().dtype(), &DataType::Int64);
        op.validate_tensors(&[tensor(&[2, 5, 6, 7, 8])], &outputs)
            .unwrap();

        let inputs = op.create_inputs(&outputs, 1.0, &mut rng).unwrap();
        assert_eq!(
            inputs[0].as_ref().unwrap().shape(),
            &Shape::new(&[2, 5, 6, 7, 8])
        );
    }

    #[test]
    fn create_outputs_with_expand_ratio_zero_omits_indices() {
        let op = OperatorMaxPool {
            strides: Some(vec![2, 2]),
            ..OperatorMaxPool::new(&[2, 2])
        };
        let mut rng = rand::rng();

        let outputs = op
            .create_outputs(&[tensor(&[1, 3, 4, 4])], 0.0, &mut rng)
            .unwrap();

        assert_eq!(outputs.len(), 1);
        assert_eq!(
            outputs[0].as_ref().unwrap().shape(),
            &Shape::new(&[1, 3, 2, 2])
        );
        assert_eq!(outputs[0].as_ref().unwrap().dtype(), &DataType::Bf16);
    }

    #[test]
    fn validate_accepts_optional_indices_output() {
        let op = OperatorMaxPool {
            strides: Some(vec![2, 2]),
            ..OperatorMaxPool::new(&[2, 2])
        };

        op.validate_tensors(&[tensor(&[1, 3, 4, 4])], &[tensor(&[1, 3, 2, 2])])
            .unwrap();
        op.validate_tensors(
            &[tensor(&[1, 3, 4, 4])],
            &[tensor(&[1, 3, 2, 2]), indices(&[1, 3, 2, 2])],
        )
        .unwrap();
    }

    #[test]
    fn validate_rejects_wrong_indices_shape_or_dtype() {
        let op = OperatorMaxPool {
            strides: Some(vec![2, 2]),
            ..OperatorMaxPool::new(&[2, 2])
        };

        let err = op
            .validate_tensors(
                &[tensor(&[1, 3, 4, 4])],
                &[tensor(&[1, 3, 2, 2]), indices(&[1, 3, 2, 1])],
            )
            .unwrap_err();
        assert!(format!("{err}").contains("Indices shape"));

        let err = op
            .validate_tensors(
                &[tensor(&[1, 3, 4, 4])],
                &[tensor(&[1, 3, 2, 2]), tensor(&[1, 3, 2, 2])],
            )
            .unwrap_err();
        assert!(format!("{err}").contains("Indices dtype"));
    }

    #[test]
    fn output_shape_supports_padding_dilation_and_ceil_mode() {
        let op = OperatorMaxPool {
            ceil_mode: Some(true),
            dilations: Some(vec![2, 2]),
            pads: Some(vec![1, 1, 1, 1]),
            strides: Some(vec![2, 2]),
            ..OperatorMaxPool::new(&[3, 3])
        };
        let mut rng = rand::rng();

        let outputs = op
            .create_outputs(&[tensor(&[1, 1, 7, 7])], 1.0, &mut rng)
            .unwrap();

        assert_eq!(
            outputs[0].as_ref().unwrap().shape(),
            &Shape::new(&[1, 1, 3, 3])
        );
    }

    #[test]
    fn output_shape_supports_same_upper_auto_pad() {
        let op = OperatorMaxPool {
            auto_pad: Some(AutoPad::SameUpper),
            strides: Some(vec![2, 2]),
            ..OperatorMaxPool::new(&[3, 3])
        };
        let mut rng = rand::rng();

        let outputs = op
            .create_outputs(&[tensor(&[1, 1, 5, 6])], 1.0, &mut rng)
            .unwrap();

        assert_eq!(
            outputs[0].as_ref().unwrap().shape(),
            &Shape::new(&[1, 1, 3, 3])
        );
    }

    #[test]
    fn delay_counts_comparisons_excluding_padding() {
        let op = OperatorMaxPool {
            pads: Some(vec![1, 1, 1, 1]),
            ..OperatorMaxPool::new(&[3, 3])
        };
        let compute_capabilities = Rc::new(ComputeCapabilities {
            adds_per_tick: 200.0,
            muls_per_tick: 100.0,
            compares_per_tick: 0.5,
            sram_bytes: 1024,
        });

        let delay = op
            .compute_delay_ticks(
                &compute_capabilities,
                &[tensor_view(&[1, 1, 2, 2])],
                &[tensor_view(&[1, 1, 2, 2]), indices_view(&[1, 1, 2, 2])],
            )
            .unwrap();

        // Every output window sees the same four real input elements, requiring
        // three comparisons per output.
        assert_eq!(delay, 24);
    }

    #[test]
    fn partitions_include_both_outputs() {
        let op = OperatorMaxPool {
            strides: Some(vec![2, 2]),
            ..OperatorMaxPool::new(&[2, 2])
        };
        let inputs = vec![tensor(&[2, 1, 4, 4])];
        let outputs = vec![tensor(&[2, 1, 2, 2]), indices(&[2, 1, 2, 2])];

        let partitions = partition_tensors(&op, &inputs, &outputs, 2).unwrap();

        let expected: &[PartitionOffsetsShapes] = &[
            (
                (&[0, 0, 0, 0], &[1, 1, 4, 4]),
                (&[0, 0, 0, 0], &[1, 1, 2, 2]),
                (&[0, 0, 0, 0], &[1, 1, 2, 2]),
            ),
            (
                (&[1, 0, 0, 0], &[1, 1, 4, 4]),
                (&[1, 0, 0, 0], &[1, 1, 2, 2]),
                (&[1, 0, 0, 0], &[1, 1, 2, 2]),
            ),
        ];
        check_partitions(&partitions, expected);
    }
}
