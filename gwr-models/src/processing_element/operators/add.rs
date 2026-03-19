// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! The Add operator
//!
//! See <https://onnx.ai/onnx/operators/onnx__Add.html#l-onnx-doc-add>

use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use rand::Rng;

use super::{Operator, Shape, Tensor, TensorPartition};
use crate::processing_element::ComputeCapabilities;
use crate::processing_element::operators::{
    HasShape, TensorView, apply_dim_partitions, partition_across_dimensions,
};

const NAME: &str = "Add";

fn choose_partition_dims<T: HasShape>(output: &T) -> Vec<usize> {
    let dims = output.shape().get_dims();
    let mut candidate_dims = dims
        .iter()
        .enumerate()
        .filter_map(|(dim, size)| (*size > 1).then_some(dim))
        .collect::<Vec<_>>();

    if candidate_dims.is_empty() {
        candidate_dims.push(output.num_dims().saturating_sub(1));
    }

    candidate_dims
}

pub struct OperatorAdd {}

fn broadcast_shapes(a: &Shape, b: &Shape) -> Result<Shape, SimError> {
    let rank_a = a.num_dims();
    let rank_b = b.num_dims();
    let rank = rank_a.max(rank_b);
    let mut result = vec![1; rank];

    for (i, result_i) in result.iter_mut().enumerate() {
        let a_dim = a.get_dim(rank, i);
        let b_dim = b.get_dim(rank, i);

        *result_i = if a_dim == b_dim {
            a_dim
        } else if a_dim == 1 {
            b_dim
        } else if b_dim == 1 {
            a_dim
        } else {
            return sim_error!("{NAME}: cannot broadcast shapes {:?} and {:?}", a, b);
        };
    }

    Ok(Shape(result))
}

fn choose_input_shape(output: &Tensor, rng: &mut impl Rng, expand_ratio: f64) -> Shape {
    let keep_prob = expand_ratio.clamp(0.0, 1.0);
    let mut dims = output.shape.0.clone();

    for dim in &mut dims {
        if *dim > 1 && rng.random_bool(1.0 - keep_prob) {
            *dim = 1;
        }
    }

    while dims.len() > 1 && dims[0] == 1 && rng.random_bool(1.0 - keep_prob) {
        dims.remove(0);
    }

    Shape(dims)
}

fn validate_inputs<T: HasShape>(inputs: &[Option<T>]) -> Result<(&T, &T), SimError> {
    if inputs.len() != 2 {
        return sim_error!("{NAME}: {} inputs found - expected 2", inputs.len());
    }
    let input_a = inputs[0]
        .as_ref()
        .ok_or(SimError(format!("{NAME}: missing input 0")))?;
    let input_b = inputs[1]
        .as_ref()
        .ok_or(SimError(format!("{NAME}: missing input 1")))?;
    Ok((input_a, input_b))
}

fn validate_outputs<T: HasShape>(outputs: &[Option<T>]) -> Result<&T, SimError> {
    if outputs.len() != 1 {
        return sim_error!("{NAME}: {} outputs found - expected 1", outputs.len());
    }
    outputs[0]
        .as_ref()
        .ok_or(SimError(format!("{NAME}: missing output")))
}

fn validate_input_outputs<'a, 'b, T: HasShape>(
    inputs: &'a [Option<T>],
    outputs: &'b [Option<T>],
) -> Result<(&'a T, &'a T, &'b T), SimError> {
    let (input_a, input_b) = validate_inputs(inputs)?;
    let output = validate_outputs(outputs)?;

    let expected_shape = broadcast_shapes(input_a.shape(), input_b.shape())?;
    if expected_shape != *output.shape() {
        return sim_error!(
            "{NAME}: Invalid output shape - expected {:?}, found {:?}",
            expected_shape,
            output.shape()
        );
    }
    Ok((input_a, input_b, output))
}

impl Operator for OperatorAdd {
    fn validate_tensors(&self, inputs: &[Option<Tensor>], outputs: &[Option<Tensor>]) -> SimResult {
        validate_input_outputs(inputs, outputs)?;
        Ok(())
    }

    fn create_outputs(
        &self,
        inputs: &[Option<Tensor>],
        _expand_ratio: f64,
        _rng: &mut impl Rng,
    ) -> Result<Vec<Option<Tensor>>, gwr_engine::types::SimError> {
        let (input_a, input_b) = validate_inputs(inputs)?;

        let output_shape = broadcast_shapes(&input_a.shape, &input_b.shape)?;
        let output_dtype = if input_a.dtype > input_b.dtype {
            input_a.dtype.clone()
        } else {
            input_b.dtype.clone()
        };

        Ok(vec![Some(Tensor {
            shape: output_shape,
            dtype: output_dtype,
            addr: 0,
        })])
    }

    fn create_inputs(
        &self,
        outputs: &[Option<Tensor>],
        expand_ratio: f64,
        rng: &mut impl Rng,
    ) -> Result<Vec<Option<Tensor>>, gwr_engine::types::SimError> {
        let output = validate_outputs(outputs)?;

        // We cannot shrink both inputs because we need to preserve one that will
        // cause the output to be of the right shape. So we choose one and update
        // that.
        let (input_a_shape, input_b_shape) = if rng.random_bool(0.5) {
            (
                choose_input_shape(output, rng, expand_ratio),
                output.shape.clone(),
            )
        } else {
            (
                output.shape.clone(),
                choose_input_shape(output, rng, expand_ratio),
            )
        };

        Ok(vec![
            Some(Tensor {
                shape: input_a_shape,
                dtype: output.dtype.clone(),
                addr: 0,
            }),
            Some(Tensor {
                shape: input_b_shape,
                dtype: output.dtype.clone(),
                addr: 0,
            }),
        ])
    }

    fn compute_delay_ticks(
        &self,
        compute_capabilities: &Rc<ComputeCapabilities>,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) -> Result<usize, SimError> {
        validate_input_outputs(inputs, outputs)?;
        let num_adds = outputs[0].as_ref().unwrap().num_elements();
        let compute_ticks = num_adds.div_ceil(compute_capabilities.adds_per_tick);
        Ok(compute_ticks)
    }

    fn partition_views(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
        num_partitions: usize,
    ) -> Result<Vec<TensorPartition>, SimError> {
        let (_, _, output_view) = validate_input_outputs(input_views, output_views)?;

        let partition_dims = choose_partition_dims(&output_view);
        let output_view_dims = output_view.shape().get_dims();
        let partition_specs =
            partition_across_dimensions(output_view_dims, &partition_dims, num_partitions);
        let output_rank = output_view.num_dims();

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
            let output_view =
                TensorView::new(output_view.tensor().clone(), &output_shape, &output_offsets);

            let input_views = input_views
                .iter()
                .map(|maybe_input_view| {
                    maybe_input_view.as_ref().map(|input_view| {
                        TensorView::from_output_partitions_on_view(input_view, output_rank, &spec)
                    })
                })
                .collect::<Vec<_>>();

            partitions.push(TensorPartition {
                inputs: input_views,
                outputs: vec![Some(output_view)],
            });
        }

        Ok(partitions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing_element::operators::dtype::DataType;
    use crate::processing_element::operators::partition_tensors;

    fn tensor(dims: &[usize]) -> Option<Tensor> {
        Some(Tensor::new(dims, &DataType::Bf16, 0))
    }

    fn tensor_view(dims: &[usize]) -> Option<TensorView> {
        let tensor = Tensor::new(dims, &DataType::Bf16, 0);
        Some(TensorView::new_full(tensor))
    }

    #[test]
    fn create_outputs_broadcasts_same_rank_inputs() {
        let op = OperatorAdd {};
        let inputs = vec![tensor(&[2, 3, 4]), tensor(&[1, 3, 1])];
        let mut rng = rand::rng();

        let outputs = op.create_outputs(&inputs, 1.0, &mut rng).unwrap();

        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].as_ref().unwrap().shape, Shape(vec![2, 3, 4]));
    }

    #[test]
    fn create_outputs_broadcasts_different_rank_inputs() {
        let op = OperatorAdd {};
        let inputs = vec![tensor(&[3, 4]), tensor(&[2, 1, 4])];
        let mut rng = rand::rng();

        let outputs = op.create_outputs(&inputs, 1.0, &mut rng).unwrap();

        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].as_ref().unwrap().shape, Shape(vec![2, 3, 4]));
    }

    #[test]
    fn create_outputs_rejects_non_broadcastable_inputs() {
        let op = OperatorAdd {};
        let inputs = vec![tensor(&[2, 3]), tensor(&[4, 3])];
        let mut rng = rand::rng();

        let err = op.create_outputs(&inputs, 1.0, &mut rng).unwrap_err();

        assert!(format!("{err}").contains("cannot broadcast shapes"));
    }

    #[test]
    fn validate_tensors_accepts_broadcasted_output_shape() {
        let op = OperatorAdd {};
        let inputs = vec![tensor(&[3, 4]), tensor(&[2, 1, 4])];
        let outputs = vec![tensor(&[2, 3, 4])];

        op.validate_tensors(&inputs, &outputs).unwrap();
    }

    #[test]
    fn validate_tensors_rejects_wrong_output_shape() {
        let op = OperatorAdd {};
        let inputs = vec![tensor(&[3, 4]), tensor(&[2, 1, 4])];
        let outputs = vec![tensor(&[3, 4])];

        let err = op.validate_tensors(&inputs, &outputs).unwrap_err();

        assert!(format!("{err}").contains("Invalid output shape"));
    }

    #[test]
    fn create_inputs_with_expand_ratio_one_preserves_output_shape() {
        let op = OperatorAdd {};
        let outputs = vec![tensor(&[2, 3, 4])];
        let mut rng = rand::rng();

        let inputs = op.create_inputs(&outputs, 1.0, &mut rng).unwrap();

        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].as_ref().unwrap().shape, Shape(vec![2, 3, 4]));
        assert_eq!(inputs[1].as_ref().unwrap().shape, Shape(vec![2, 3, 4]));
        op.validate_tensors(&inputs, &outputs).unwrap();
    }

    #[test]
    fn create_inputs_with_expand_ratio_zero_creates_broadcastable_inputs() {
        let op = OperatorAdd {};
        let outputs = vec![tensor(&[2, 3, 4])];
        let mut rng = rand::rng();

        let inputs = op.create_inputs(&outputs, 0.0, &mut rng).unwrap();

        assert_eq!(inputs.len(), 2);
        op.validate_tensors(&inputs, &outputs).unwrap();

        let inputs: Vec<Tensor> = inputs.into_iter().map(|input| input.unwrap()).collect();
        let outputs: Vec<Tensor> = outputs.into_iter().map(|output| output.unwrap()).collect();
        assert!(inputs[0].num_dims() <= outputs[0].num_dims());
        assert!(inputs[1].num_dims() <= outputs[0].num_dims());
        assert!(
            inputs[0]
                .shape
                .0
                .iter()
                .all(|dim| *dim == 1 || outputs[0].shape.0.contains(dim))
        );
        assert!(
            inputs[1]
                .shape
                .0
                .iter()
                .all(|dim| *dim == 1 || outputs[0].shape.0.contains(dim))
        );
    }

    #[test]
    fn delay_ticks() {
        let compute_capabilities = Rc::new(ComputeCapabilities {
            adds_per_tick: 1,
            muls_per_tick: 100,
            sram_bytes: 1024,
        });
        let operator = OperatorAdd {};
        let delay_ticks = operator
            .compute_delay_ticks(
                &compute_capabilities,
                &[tensor_view(&[4, 5]), tensor_view(&[4, 5])],
                &[tensor_view(&[4, 5])],
            )
            .unwrap();
        assert_eq!(delay_ticks, 20);

        let compute_capabilities = Rc::new(ComputeCapabilities {
            adds_per_tick: 2,
            muls_per_tick: 100,
            sram_bytes: 1024,
        });
        let delay_ticks = operator
            .compute_delay_ticks(
                &compute_capabilities,
                &[tensor_view(&[4, 5]), tensor_view(&[4, 5])],
                &[tensor_view(&[4, 5])],
            )
            .unwrap();
        assert_eq!(delay_ticks, 10);

        let delay_ticks = operator
            .compute_delay_ticks(
                &compute_capabilities,
                &[tensor_view(&[10, 4, 5]), tensor_view(&[10, 4, 5])],
                &[tensor_view(&[10, 4, 5])],
            )
            .unwrap();
        assert_eq!(delay_ticks, 100);
    }

    type ShapeOffsets = (&'static [usize], &'static [usize]);

    // Ensure that both inputs and the output are all of the expected shape as they
    // should all be the same.
    fn check_partitions(partitions: &[TensorPartition], expected: &[ShapeOffsets]) {
        assert_eq!(partitions.len(), expected.len());

        for (partition, (expected_offsets, expected_shape)) in
            partitions.iter().zip(expected.iter())
        {
            let views = partition
                .inputs
                .iter()
                .chain(partition.outputs.iter())
                .map(|view| view.as_ref().unwrap());

            for view in views {
                assert_eq!(view.offsets().get_dims().as_slice(), *expected_offsets);
                assert_eq!(view.shape().get_dims().as_slice(), *expected_shape);
            }
        }
    }

    #[test]
    fn can_partition_across_one_dimension() {
        let op = OperatorAdd {};
        let inputs = vec![tensor(&[1, 5, 3, 4]), tensor(&[1, 5, 3, 4])];
        let outputs = vec![tensor(&[1, 5, 3, 4])];

        let partitions = partition_tensors(&op, &inputs, &outputs, 5).unwrap();
        assert_eq!(partitions.len(), 5);

        let expected: &[ShapeOffsets] = &[
            (&[0, 0, 0, 0], &[1, 1, 3, 4]),
            (&[0, 1, 0, 0], &[1, 1, 3, 4]),
            (&[0, 2, 0, 0], &[1, 1, 3, 4]),
            (&[0, 3, 0, 0], &[1, 1, 3, 4]),
            (&[0, 4, 0, 0], &[1, 1, 3, 4]),
        ];
        check_partitions(&partitions, expected);
    }

    #[test]
    fn can_partition_across_multiple_dimensions() {
        let op = OperatorAdd {};
        let inputs = vec![tensor(&[2, 3, 4]), tensor(&[2, 3, 4])];
        let outputs = vec![tensor(&[2, 3, 4])];

        let partitions = partition_tensors(&op, &inputs, &outputs, 5).unwrap();
        assert_eq!(partitions.len(), 6);

        let expected: &[ShapeOffsets] = &[
            (&[0, 0, 0], &[1, 1, 4]),
            (&[0, 1, 0], &[1, 1, 4]),
            (&[0, 2, 0], &[1, 1, 4]),
            (&[1, 0, 0], &[1, 1, 4]),
            (&[1, 1, 0], &[1, 1, 4]),
            (&[1, 2, 0], &[1, 1, 4]),
        ];
        check_partitions(&partitions, expected);
    }

    #[test]
    fn partitions_preserve_subset_view_offsets() {
        let op = OperatorAdd {};
        let input_a = Tensor::new(&[4, 5, 4], &DataType::Bf16, 0);
        let input_b = Tensor::new(&[4, 5, 4], &DataType::Bf16, 0);
        let output = Tensor::new(&[4, 5, 4], &DataType::Bf16, 0);

        let input_views = vec![
            Some(TensorView::new(input_a, &[2, 2, 4], &[1, 2, 0])),
            Some(TensorView::new(input_b, &[2, 2, 4], &[1, 2, 0])),
        ];
        let output_views = vec![Some(TensorView::new(output, &[2, 2, 4], &[1, 2, 0]))];

        let partitions = op.partition_views(&input_views, &output_views, 2).unwrap();
        assert_eq!(partitions.len(), 2);

        let expected: &[ShapeOffsets] = &[(&[1, 2, 0], &[1, 2, 4]), (&[2, 2, 0], &[1, 2, 4])];
        check_partitions(&partitions, expected);
    }
}
