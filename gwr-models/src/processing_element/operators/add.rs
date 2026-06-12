// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! The Add operator
//!
//! See <https://onnx.ai/onnx/operators/onnx__Add.html#l-onnx-doc-add>

use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use rand::Rng;

use super::{Operator, Shape, Tensor, TensorPartition};
use crate::processing_element::operators::{
    HasShape, TensorView, apply_dim_partitions, partition_across_dimensions,
};
use crate::processing_element::{ComputeCapabilities, MachineOp};

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

fn num_add_flops<T: HasShape>(
    inputs: &[Option<T>],
    outputs: &[Option<T>],
) -> Result<usize, SimError> {
    let (_, _, output) = validate_input_outputs(inputs, outputs)?;
    Ok(output.num_elements())
}

impl OperatorAdd {
    pub fn create_outputs(
        &self,
        inputs: &[Option<Tensor>],
        _expand_ratio: f64,
        _rng: &mut impl Rng,
    ) -> Result<Vec<Option<Tensor>>, gwr_engine::types::SimError> {
        let (input_a, input_b) = validate_inputs(inputs)?;

        let output_shape = broadcast_shapes(&input_a.shape, &input_b.shape)?;
        let output_dtype = if input_a.dtype > input_b.dtype {
            input_a.dtype
        } else {
            input_b.dtype
        };

        Ok(vec![Some(Tensor {
            id: None,
            shape: output_shape,
            dtype: output_dtype,
            addr: 0,
        })])
    }

    pub fn create_inputs(
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
                id: None,
                shape: input_a_shape,
                dtype: output.dtype,
                addr: 0,
            }),
            Some(Tensor {
                id: None,
                shape: input_b_shape,
                dtype: output.dtype,
                addr: 0,
            }),
        ])
    }
}

impl Operator for OperatorAdd {
    fn validate_tensors(&self, inputs: &[Option<Tensor>], outputs: &[Option<Tensor>]) -> SimResult {
        validate_input_outputs(inputs, outputs)?;
        Ok(())
    }

    fn compute_delay_ticks(
        &self,
        compute_capabilities: &Rc<ComputeCapabilities>,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) -> Result<usize, SimError> {
        let num_adds = num_add_flops(inputs, outputs)?;
        compute_capabilities.cycles_for_ops(num_adds, MachineOp::Add)
    }

    fn compute_flops(
        &self,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) -> Result<usize, SimError> {
        num_add_flops(inputs, outputs)
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
    use rand::RngCore;

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

    struct FixedBoolRng {
        values: Vec<bool>,
    }

    impl FixedBoolRng {
        fn with_bool_values(values: impl IntoIterator<Item = bool>) -> Self {
            Self {
                values: values.into_iter().collect(),
            }
        }

        fn next_bool(&mut self) -> bool {
            if self.values.is_empty() {
                true
            } else {
                self.values.remove(0)
            }
        }
    }

    impl RngCore for FixedBoolRng {
        fn next_u32(&mut self) -> u32 {
            if self.next_bool() { 0 } else { u32::MAX }
        }

        fn next_u64(&mut self) -> u64 {
            if self.next_bool() { 0 } else { u64::MAX }
        }

        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for chunk in dst.chunks_mut(size_of::<u64>()) {
                let bytes = self.next_u64().to_le_bytes();
                chunk.copy_from_slice(&bytes[..chunk.len()]);
            }
        }
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

        for shrink_input_a in [false, true] {
            let mut rng = FixedBoolRng::with_bool_values([shrink_input_a]);

            let inputs = op.create_inputs(&outputs, 1.0, &mut rng).unwrap();

            assert_eq!(inputs.len(), 2);
            assert_eq!(inputs[0].as_ref().unwrap().shape, Shape(vec![2, 3, 4]));
            assert_eq!(inputs[1].as_ref().unwrap().shape, Shape(vec![2, 3, 4]));
            op.validate_tensors(&inputs, &outputs).unwrap();
        }
    }

    #[test]
    fn create_inputs_with_expand_ratio_zero_creates_broadcastable_inputs() {
        let op = OperatorAdd {};
        let outputs = vec![tensor(&[2, 3, 4])];

        for shrink_input_a in [false, true] {
            let mut rng = FixedBoolRng::with_bool_values([shrink_input_a]);

            let inputs = op.create_inputs(&outputs, 0.0, &mut rng).unwrap();

            assert_eq!(inputs.len(), 2);
            op.validate_tensors(&inputs, &outputs).unwrap();

            let inputs: Vec<Tensor> = inputs.into_iter().map(|input| input.unwrap()).collect();
            let outputs: Vec<Tensor> = outputs
                .iter()
                .map(|output| output.clone().unwrap())
                .collect();
            assert_eq!(inputs[usize::from(shrink_input_a)].shape, outputs[0].shape);
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
    }

    #[test]
    fn delay_ticks() {
        let compute_capabilities = Rc::new(ComputeCapabilities {
            adds_per_tick: 1.0,
            muls_per_tick: 100.0,
            compares_per_tick: 200.0,
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
            adds_per_tick: 2.0,
            muls_per_tick: 100.0,
            compares_per_tick: 100.0,
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

    #[test]
    fn flop_count_matches_output_elements() {
        let operator = OperatorAdd {};
        assert_eq!(
            operator
                .compute_flops(
                    &[tensor_view(&[2, 3, 4]), tensor_view(&[1, 3, 1])],
                    &[tensor_view(&[2, 3, 4])],
                )
                .unwrap(),
            24
        );
    }

    type OffsetsShapes = (&'static [usize], &'static [usize]);

    // Ensure that both inputs and the output are all of the expected shape as they
    // should all be the same.
    fn check_partitions(partitions: &[TensorPartition], expected: &[OffsetsShapes]) {
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

        let expected: &[OffsetsShapes] = &[
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

        let expected: &[OffsetsShapes] = &[
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

        let expected: &[OffsetsShapes] = &[(&[1, 2, 0], &[1, 2, 4]), (&[2, 2, 0], &[1, 2, 4])];
        check_partitions(&partitions, expected);
    }
}
