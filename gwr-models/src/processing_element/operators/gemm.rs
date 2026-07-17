// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! The Gemm operator
//!
//! See <https://onnx.ai/onnx/operators/onnx__Gemm.html#l-onnx-doc-gemm>

use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use rand::Rng;

use super::{Operator, Tensor, TensorPartition};
use crate::processing_element::operators::{
    HasShape, Shape, TensorView, apply_dim_partitions, partition_across_dimensions,
};
use crate::processing_element::{ComputeCapabilities, MachineOp, MachineOpCounts};

const NAME: &str = "Gemm";

/// Return all dimensions that can be partitioned.
///
/// Prefer outer dims first. Add M and N in case required.
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

// Define offsets from the inner-most dimension for named dimensions
const INPUT_A_OFFSET_M: usize = 2;
const INPUT_A_OFFSET_K: usize = 1;
const INPUT_B_OFFSET_K: usize = 2;
const INPUT_B_OFFSET_N: usize = 1;
const OUTPUT_OFFSET_M: usize = 2;
const OUTPUT_OFFSET_N: usize = 1;

/// Return the value of a dimension starting from the inner most
fn get_inner_dim<T: HasShape>(has_shape: &T, i: usize) -> usize {
    has_shape.shape().get_dims()[has_shape.num_dims() - i].max(1)
}

/// Construct an input B shape for a Gemm consuming `input_a` as input A.
pub fn gemm_rhs_shape<T: HasShape>(input_a: &T) -> Result<Shape, SimError> {
    let rank = input_a.num_dims();
    if rank < 2 {
        return sim_error!("{NAME}: input A must be at least 2D");
    }

    let mut rhs_dims = input_a.shape().get_dims().clone();
    rhs_dims[rank - INPUT_B_OFFSET_K] = get_inner_dim(input_a, INPUT_A_OFFSET_K);
    rhs_dims[rank - INPUT_B_OFFSET_N] = get_inner_dim(input_a, INPUT_A_OFFSET_M);
    Ok(Shape::new(&rhs_dims))
}

/// Choose a value for the K in a (M,K)x(K,N) -> (M,N) Gemm
///
/// Starts with the maximum of M and N and then grows or shrinks depending
/// on the `expand_ratio` specified
fn choose_gemm_k(output: &Tensor, rng: &mut impl Rng, expand_ratio: f64) -> usize {
    let m = get_inner_dim(output, OUTPUT_OFFSET_M);
    let n = get_inner_dim(output, OUTPUT_OFFSET_N);
    let reference = m.max(n);
    let scaled = ((reference as f64) * expand_ratio).round().max(1.0) as usize;
    let lower = ((scaled as f64) * 0.75).round().max(1.0) as usize;
    let upper = ((scaled as f64) * 1.25).round().max(lower as f64) as usize;

    if lower == upper {
        lower
    } else {
        rng.random_range(lower..=upper)
    }
}

fn should_add_input_c(rng: &mut impl Rng, expand_ratio: f64) -> bool {
    if !expand_ratio.is_finite() || expand_ratio <= 0.0 {
        false
    } else if expand_ratio >= 1.0 {
        true
    } else {
        rng.random_bool(expand_ratio)
    }
}

fn output_tensor_from_inputs(inputs: &[Option<Tensor>]) -> Result<Tensor, SimError> {
    let (input_a, input_b) = validate_inputs(inputs)?;

    let output_shape = broadcast_shapes(&input_a.shape, &input_b.shape)?;
    let output_dtype = if input_a.dtype > input_b.dtype {
        input_a.dtype
    } else {
        input_b.dtype
    };

    Ok(Tensor {
        id: None,
        shape: output_shape,
        dtype: output_dtype,
        addr: 0,
    })
}

pub fn maybe_add_input_c(
    inputs: &mut Vec<Option<Tensor>>,
    expand_ratio: f64,
    rng: &mut impl Rng,
) -> Result<bool, SimError> {
    if inputs.len() >= 3 || !should_add_input_c(rng, expand_ratio) {
        return Ok(false);
    }

    inputs.push(Some(output_tensor_from_inputs(inputs)?));
    Ok(true)
}

fn broadcast_shapes(a: &Shape, b: &Shape) -> Result<Shape, SimError> {
    let rank_a = a.num_dims();
    let rank_b = b.num_dims();
    let rank_result = rank_a.max(rank_b);

    if rank_result < 2 {
        return sim_error!("{NAME}: inputs must be at least 2D ({:?} and {:?})", a, b);
    }

    let mut result = vec![1; rank_result];

    for (i, result_i) in result.iter_mut().enumerate() {
        let a_dim = a.get_dim(rank_result, i);
        let b_dim = b.get_dim(rank_result, i);

        *result_i = if i == (rank_result.saturating_sub(OUTPUT_OFFSET_M)) {
            // M
            a_dim
        } else if i == (rank_result.saturating_sub(OUTPUT_OFFSET_N)) {
            // N
            b_dim
        } else if a_dim == b_dim {
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

fn validate_inputs<T: HasShape>(inputs: &[Option<T>]) -> Result<(&T, &T), SimError> {
    let input_a = inputs[0]
        .as_ref()
        .ok_or(SimError(format!("{NAME}: missing input 0")))?;
    let input_b = inputs[1]
        .as_ref()
        .ok_or(SimError(format!("{NAME}: missing input 1")))?;
    let shape_a = input_a.shape();
    let shape_b = input_b.shape();
    let output_shape = broadcast_shapes(shape_a, shape_b)?;

    let k_a = get_inner_dim(shape_a, INPUT_A_OFFSET_K);
    let k_b = get_inner_dim(shape_b, INPUT_B_OFFSET_K);
    if k_a != k_b {
        return sim_error!("{NAME}: incompatible K in {:?} x {:?}", shape_a, shape_b);
    }

    if inputs.len() == 2 {
        // Input C is a scalar that is broadcast - so no issues
    } else if inputs.len() == 3
        && let Some(input_c) = &inputs[2]
    {
        // Validate the Tensor C inputs
        let out_m = get_inner_dim(&output_shape, OUTPUT_OFFSET_M);
        let out_n = get_inner_dim(&output_shape, OUTPUT_OFFSET_N);

        let shape_c = input_c.shape();
        let c_m = get_inner_dim(&shape_c, OUTPUT_OFFSET_M);
        let c_n = get_inner_dim(&shape_c, OUTPUT_OFFSET_N);
        if (c_m != 1 && c_m != out_m) || (c_n != 1 && c_n != out_n) {
            return sim_error!(
                "{NAME}: input C incompatible ({:?} x {:?}) + {:?}",
                shape_a,
                shape_b,
                shape_c
            );
        }
    } else {
        return sim_error!(
            "{NAME}: {} input tensors found - expected 2 or 3",
            inputs.len()
        );
    }

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

    let shape_a = input_a.shape();
    let shape_b = input_b.shape();
    let rank_inputs = input_a.num_dims().max(input_b.num_dims());
    let shape_output = output.shape();
    let rank_output = output.num_dims();

    if rank_inputs != rank_output {
        return sim_error!(
            "{NAME}: incompatible ranks ({:?} x {:?} => {:?}",
            shape_a,
            shape_b,
            shape_output
        );
    }

    // We just need to check input vs output as the inputs have already been
    // validated against each other
    for i in 0..(rank_inputs - 2) {
        let dim_a = input_a.get_dim(rank_inputs, i);
        let dim_b = input_b.get_dim(rank_inputs, i);
        let dim_in = if dim_a == 1 { dim_b } else { dim_a };
        let dim_out = output.get_dim(rank_output, i);
        if dim_in != 1 && dim_out != dim_in {
            return sim_error!(
                "{NAME}: Invalid output dimension {i} in {:?} x {:?} => {:?}",
                shape_a,
                shape_b,
                shape_output
            );
        }
    }

    let input_m = get_inner_dim(input_a, INPUT_A_OFFSET_M);
    let input_n = get_inner_dim(input_b, INPUT_B_OFFSET_N);
    let output_m = get_inner_dim(output, OUTPUT_OFFSET_M);
    let output_n = get_inner_dim(output, OUTPUT_OFFSET_N);

    if (input_m != output_m) || (input_n != output_n) {
        return sim_error!(
            "{NAME}: Invalid M or N {:?} x {:?} => {:?}",
            shape_a,
            shape_b,
            shape_output
        );
    }

    Ok((input_a, input_b, output))
}

fn gemm_op_counts<T: HasShape>(
    inputs: &[Option<T>],
    outputs: &[Option<T>],
) -> Result<(usize, usize), SimError> {
    let (input_a_view, input_b_view, output_view) = validate_input_outputs(inputs, outputs)?;
    let m = get_inner_dim(input_a_view, INPUT_A_OFFSET_M);
    let k = get_inner_dim(input_a_view, INPUT_A_OFFSET_K);
    let n = get_inner_dim(input_b_view, INPUT_B_OFFSET_N);

    let num_matmuls = output_view
        .shape()
        .get_dims()
        .iter()
        .take(output_view.num_dims().saturating_sub(2))
        .product::<usize>();

    let num_muls = m * n * k * num_matmuls;
    let num_matmul_adds = m * n * (k - 1) * num_matmuls;
    // When there is a C input tensor each output element has one extra add
    let num_c_adds = usize::from(inputs.len() == 3) * output_view.shape().num_elements();
    let num_adds = num_matmul_adds + num_c_adds;
    Ok((num_muls, num_adds))
}

pub struct OperatorGemm {}

impl OperatorGemm {
    pub fn create_outputs(
        &self,
        inputs: &[Option<Tensor>],
        _expand_ratio: f64,
        _rng: &mut impl Rng,
    ) -> Result<Vec<Option<Tensor>>, gwr_engine::types::SimError> {
        Ok(vec![Some(output_tensor_from_inputs(inputs)?)])
    }

    pub fn create_inputs(
        &self,
        outputs: &[Option<Tensor>],
        expand_ratio: f64,
        rng: &mut impl Rng,
    ) -> Result<Vec<Option<Tensor>>, gwr_engine::types::SimError> {
        let output = validate_outputs(outputs)?;

        let mut input_a_shape = output.shape.clone();
        let mut input_b_shape = output.shape.clone();

        let k = choose_gemm_k(output, rng, expand_ratio);

        let rank = output.num_dims();
        input_a_shape.0[rank.saturating_sub(INPUT_A_OFFSET_K)] = k;
        input_b_shape.0[rank.saturating_sub(INPUT_B_OFFSET_K)] = k;

        let mut inputs = vec![
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
        ];

        maybe_add_input_c(&mut inputs, expand_ratio, rng)?;

        Ok(inputs)
    }
}

impl Operator for OperatorGemm {
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
        let (num_muls, num_adds) = gemm_op_counts(inputs, outputs)?;
        Ok(
            compute_capabilities.cycles_for_ops(num_muls, MachineOp::Mul)?
                + compute_capabilities.cycles_for_ops(num_adds, MachineOp::Add)?,
        )
    }

    fn compute_machine_ops(
        &self,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) -> Result<MachineOpCounts, SimError> {
        let (num_muls, num_adds) = gemm_op_counts(inputs, outputs)?;
        Ok(MachineOpCounts {
            adds: num_adds,
            muls: num_muls,
            ..MachineOpCounts::default()
        })
    }

    fn partition_views(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
        num_partitions: usize,
    ) -> Result<Vec<TensorPartition>, SimError> {
        let (input_a_view, input_b_view, output_view) =
            validate_input_outputs(input_views, output_views)?;

        let input_c_view = if input_views.len() > 2 {
            input_views[2].clone()
        } else {
            None
        };

        let rank = output_view.num_dims();
        let partition_dims = choose_partition_dims(&output_view);
        let output_view_dims = output_view.shape().get_dims();
        let partition_specs =
            partition_across_dimensions(output_view_dims, &partition_dims, num_partitions);
        let m_dim = rank.saturating_sub(OUTPUT_OFFSET_M);
        let n_dim = rank.saturating_sub(OUTPUT_OFFSET_N);

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

            let a_spec = spec
                .iter()
                .filter(|partition| partition.dim != n_dim)
                .cloned()
                .collect::<Vec<_>>();
            let b_spec = spec
                .iter()
                .filter(|partition| partition.dim != m_dim)
                .cloned()
                .collect::<Vec<_>>();

            let split_m = spec.iter().any(|partition| partition.dim == m_dim);
            let split_n = spec.iter().any(|partition| partition.dim == n_dim);
            let split_outer = spec
                .iter()
                .any(|partition| partition.dim != m_dim && partition.dim != n_dim);

            let input_a_view = if split_outer || split_m {
                TensorView::from_output_partitions_on_view(input_a_view, rank, &a_spec)
            } else {
                input_a_view.clone()
            };

            let input_b_view = if split_outer || split_n {
                TensorView::from_output_partitions_on_view(input_b_view, rank, &b_spec)
            } else {
                input_b_view.clone()
            };

            let input_c_view = input_c_view
                .as_ref()
                .map(|view| TensorView::from_output_partitions_on_view(view, rank, &spec));

            let mut partition_inputs = vec![Some(input_a_view), Some(input_b_view)];
            if let Some(view) = input_c_view {
                partition_inputs.push(Some(view));
            }

            partitions.push(TensorPartition {
                inputs: partition_inputs,
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
    use crate::processing_element::operators::{Operator, Shape, Tensor, partition_tensors};

    fn tensor(shape: &[usize]) -> Option<Tensor> {
        Some(Tensor::new(shape, &DataType::Bf16, 0))
    }

    fn tensor_view(shape: &[usize]) -> Option<TensorView> {
        let tensor = Tensor::new(shape, &DataType::Bf16, 0);
        Some(TensorView::new_full(tensor))
    }

    #[test]
    fn test_broadcast_shapes() {
        let a = Shape(vec![1, 1, 4, 5]);
        let b = Shape(vec![1, 1, 5, 10]);
        let c = broadcast_shapes(&a, &b).unwrap();
        assert_eq!(c, Shape(vec![1, 1, 4, 10]));

        let a = Shape(vec![3, 1, 4, 5]);
        let b = Shape(vec![1, 5, 5, 10]);
        let c = broadcast_shapes(&a, &b).unwrap();
        assert_eq!(c, Shape(vec![3, 5, 4, 10]));
    }

    #[test]
    fn create_outputs_uses_gemm_m_and_n_before_broadcasting() {
        let operator = OperatorGemm {};
        let mut rng = rand::rng();

        let outputs = operator
            .create_outputs(
                &[tensor(&[1, 48, 1, 25]), tensor(&[1, 48, 25, 1])],
                1.0,
                &mut rng,
            )
            .unwrap();

        assert_eq!(
            outputs[0].as_ref().unwrap().shape(),
            &Shape::new(&[1, 48, 1, 1])
        );
    }

    #[test]
    fn validate_gemm() {
        let operator = OperatorGemm {};

        operator
            .validate_tensors(
                &[tensor(&[1, 4, 5]), tensor(&[1, 5, 8])],
                &[tensor(&[1, 4, 8])],
            )
            .unwrap();

        operator
            .validate_tensors(
                &[tensor(&[3, 2, 10, 5]), tensor(&[5, 12])],
                &[tensor(&[3, 2, 10, 12])],
            )
            .unwrap();
    }

    #[test]
    fn invalid_broadcast_1() {
        let operator = OperatorGemm {};

        let err = operator
            .validate_tensors(
                &[tensor(&[2, 4, 5]), tensor(&[3, 5, 8])],
                &[tensor(&[1, 4, 8])],
            )
            .unwrap_err();
        assert!(format!("{err}").contains("cannot broadcast shapes"));
    }

    #[test]
    fn invalid_broadcast_2() {
        let operator = OperatorGemm {};

        let err = operator
            .validate_tensors(
                &[tensor(&[4, 1, 4, 5]), tensor(&[3, 1, 5, 8])],
                &[tensor(&[1, 1, 4, 8])],
            )
            .unwrap_err();
        assert!(format!("{err}").contains("cannot broadcast shapes"));
    }

    #[test]
    fn invalid_k() {
        let operator = OperatorGemm {};

        let err = operator
            .validate_tensors(
                &[tensor(&[9, 2, 4, 5]), tensor(&[9, 2, 4, 8])],
                &[tensor(&[9, 2, 4, 8])],
            )
            .unwrap_err();
        assert!(format!("{err}").contains("incompatible K"));
    }

    #[test]
    fn get_inner_dim_uses_one_based_inner_offsets() {
        let shape = Shape::new(&[7, 11, 13, 17]);

        assert_eq!(get_inner_dim(&shape, 1), 17);
        assert_eq!(get_inner_dim(&shape, 2), 13);
        assert_eq!(get_inner_dim(&shape, 3), 11);
        assert_eq!(get_inner_dim(&shape, 4), 7);
    }

    #[test]
    fn gemm_named_offsets_map_to_expected_dimensions() {
        let input_a = Shape::new(&[19, 23, 29, 31]);
        let input_b = Shape::new(&[19, 23, 31, 37]);
        let output = Shape::new(&[19, 23, 29, 37]);

        assert_eq!(get_inner_dim(&input_a, INPUT_A_OFFSET_M), 29);
        assert_eq!(get_inner_dim(&input_a, INPUT_A_OFFSET_K), 31);
        assert_eq!(get_inner_dim(&input_b, INPUT_B_OFFSET_K), 31);
        assert_eq!(get_inner_dim(&input_b, INPUT_B_OFFSET_N), 37);
        assert_eq!(get_inner_dim(&output, OUTPUT_OFFSET_M), 29);
        assert_eq!(get_inner_dim(&output, OUTPUT_OFFSET_N), 37);
    }

    #[test]
    fn gemm_rhs_shape_uses_gemm_named_offsets() {
        let input_a = Shape::new(&[19, 23, 29, 31]);
        let input_b = gemm_rhs_shape(&input_a).unwrap();

        assert_eq!(input_b, Shape::new(&[19, 23, 31, 29]));

        OperatorGemm {}
            .validate_tensors(
                &[tensor(input_a.get_dims()), tensor(input_b.get_dims())],
                &[tensor(&[19, 23, 29, 29])],
            )
            .unwrap();
    }

    #[test]
    fn gemm_rhs_shape_rejects_rank_below_two() {
        let err = gemm_rhs_shape(&Shape::new(&[31])).unwrap_err();

        assert!(format!("{err}").contains("input A must be at least 2D"));
    }

    #[test]
    fn delay_ticks_uses_m_k_and_n_from_the_innermost_dimensions() {
        let operator = OperatorGemm {};
        let compute_capabilities = Rc::new(ComputeCapabilities {
            adds_per_tick: 1.0,
            muls_per_tick: 1.0,
            compares_per_tick: 100.0,
            sram_bytes: 1024,
        });
        let delay_ticks = operator
            .compute_delay_ticks(
                &compute_capabilities,
                &[tensor_view(&[2, 3, 4, 5]), tensor_view(&[2, 3, 5, 7])],
                &[tensor_view(&[2, 3, 4, 7])],
            )
            .unwrap();

        // Expect outer dimension GEMMS of M * K * N muls + M * (K - 1) * N adds
        assert_eq!(delay_ticks, (2 * 3) * ((4 * 5 * 7) + (4 * 4 * 7)));
    }

    #[test]
    fn delay_ticks() {
        let operator = OperatorGemm {};
        let compute_capabilities = Rc::new(ComputeCapabilities {
            adds_per_tick: 1.0,
            muls_per_tick: 1.0,
            compares_per_tick: 100.0,
            sram_bytes: 1024,
        });
        let delay_ticks = operator
            .compute_delay_ticks(
                &compute_capabilities,
                &[tensor_view(&[4, 5]), tensor_view(&[5, 8])],
                &[tensor_view(&[4, 8])],
            )
            .unwrap();
        assert_eq!(delay_ticks, 160 + 128);

        let delay_ticks = operator
            .compute_delay_ticks(
                &compute_capabilities,
                &[tensor_view(&[10, 11, 4, 5]), tensor_view(&[5, 8])],
                &[tensor_view(&[10, 11, 4, 8])],
            )
            .unwrap();
        assert_eq!(delay_ticks, 17600 + 14080);
    }

    #[test]
    fn flop_count_adds_multiplies_and_accumulates() {
        let operator = OperatorGemm {};
        assert_eq!(
            operator
                .compute_flops(
                    &[tensor_view(&[4, 5]), tensor_view(&[5, 8])],
                    &[tensor_view(&[4, 8])],
                )
                .unwrap(),
            (4 * 5 * 8) + (4 * (5 - 1) * 8)
        );
    }

    #[test]
    fn flop_count_includes_optional_c_elementwise_add() {
        let operator = OperatorGemm {};
        assert_eq!(
            operator
                .compute_flops(
                    &[
                        tensor_view(&[4, 5]),
                        tensor_view(&[5, 8]),
                        tensor_view(&[4, 8]),
                    ],
                    &[tensor_view(&[4, 8])],
                )
                .unwrap(),
            (4 * 5 * 8) + (4 * (5 - 1) * 8) + (4 * 8)
        );
    }

    #[test]
    fn create_inputs_with_expand_ratio_zero_omits_input_c() {
        let operator = OperatorGemm {};
        let mut rng = rand::rng();

        let inputs = operator
            .create_inputs(&[tensor(&[4, 8])], 0.0, &mut rng)
            .unwrap();

        assert_eq!(inputs.len(), 2);
        operator
            .validate_tensors(&inputs, &[tensor(&[4, 8])])
            .unwrap();
    }

    #[test]
    fn create_inputs_with_expand_ratio_one_adds_input_c() {
        let operator = OperatorGemm {};
        let mut rng = rand::rng();

        let inputs = operator
            .create_inputs(&[tensor(&[4, 8])], 1.0, &mut rng)
            .unwrap();

        assert_eq!(inputs.len(), 3);
        assert_eq!(inputs[2].as_ref().unwrap().shape(), &Shape::new(&[4, 8]));
        operator
            .validate_tensors(&inputs, &[tensor(&[4, 8])])
            .unwrap();
    }

    type OffsetsShapes = (&'static [usize], &'static [usize]);
    type PartitionOffsetsShapes = (OffsetsShapes, OffsetsShapes, OffsetsShapes);
    fn check_partitions(partitions: &[TensorPartition], expected: &[PartitionOffsetsShapes]) {
        assert_eq!(partitions.len(), expected.len());
        for (partition, expected_partition) in partitions.iter().zip(expected.iter()) {
            let in_a = partition.inputs[0].as_ref().unwrap();
            let in_b = partition.inputs[1].as_ref().unwrap();
            let out = partition.outputs[0].as_ref().unwrap();

            assert_eq!(
                in_a.offsets().get_dims().as_slice(),
                (expected_partition.0).0
            );
            assert_eq!(in_a.shape().get_dims().as_slice(), (expected_partition.0).1);
            assert_eq!(
                in_b.offsets().get_dims().as_slice(),
                (expected_partition.1).0
            );
            assert_eq!(in_b.shape().get_dims().as_slice(), (expected_partition.1).1);
            assert_eq!(
                out.offsets().get_dims().as_slice(),
                (expected_partition.2).0
            );
            assert_eq!(out.shape().get_dims().as_slice(), (expected_partition.2).1);
        }
    }

    #[test]
    fn partitions_prefer_outer_dims_before_m_or_n() {
        let operator = OperatorGemm {};
        let input_tensors = vec![tensor(&[3, 20, 10, 5]), tensor(&[3, 20, 5, 12])];
        let output_tensors = vec![tensor(&[3, 20, 10, 12])];

        let partitions = partition_tensors(&operator, &input_tensors, &output_tensors, 4).unwrap();
        assert_eq!(partitions.len(), 6);

        let expected: &[PartitionOffsetsShapes] = &[
            (
                (&[0, 0, 0, 0], &[1, 10, 10, 5]),
                (&[0, 0, 0, 0], &[1, 10, 5, 12]),
                (&[0, 0, 0, 0], &[1, 10, 10, 12]),
            ),
            (
                (&[0, 10, 0, 0], &[1, 10, 10, 5]),
                (&[0, 10, 0, 0], &[1, 10, 5, 12]),
                (&[0, 10, 0, 0], &[1, 10, 10, 12]),
            ),
            (
                (&[1, 0, 0, 0], &[1, 10, 10, 5]),
                (&[1, 0, 0, 0], &[1, 10, 5, 12]),
                (&[1, 0, 0, 0], &[1, 10, 10, 12]),
            ),
            (
                (&[1, 10, 0, 0], &[1, 10, 10, 5]),
                (&[1, 10, 0, 0], &[1, 10, 5, 12]),
                (&[1, 10, 0, 0], &[1, 10, 10, 12]),
            ),
            (
                (&[2, 0, 0, 0], &[1, 10, 10, 5]),
                (&[2, 0, 0, 0], &[1, 10, 5, 12]),
                (&[2, 0, 0, 0], &[1, 10, 10, 12]),
            ),
            (
                (&[2, 10, 0, 0], &[1, 10, 10, 5]),
                (&[2, 10, 0, 0], &[1, 10, 5, 12]),
                (&[2, 10, 0, 0], &[1, 10, 10, 12]),
            ),
        ];
        check_partitions(&partitions, expected);
    }

    #[test]
    fn partitions_fall_back_to_m_when_no_outer_dims_are_available() {
        let operator = OperatorGemm {};
        let inputs = vec![tensor(&[10, 5]), tensor(&[5, 12])];
        let outputs = vec![tensor(&[10, 12])];

        let partitions = partition_tensors(&operator, &inputs, &outputs, 4).unwrap();
        assert_eq!(partitions.len(), 4);

        let expected_m_offsets = [0, 3, 6, 8];
        let expected_m_lengths = [3, 3, 2, 2];

        for (partition_idx, partition) in partitions.iter().enumerate() {
            let a_view = partition.inputs[0].as_ref().unwrap();
            let b_view = partition.inputs[1].as_ref().unwrap();
            let out_view = partition.outputs[0].as_ref().unwrap();

            assert_eq!(
                a_view.shape().get_dims().as_slice(),
                &[expected_m_lengths[partition_idx], 5]
            );
            assert_eq!(
                a_view.offsets().get_dims().as_slice(),
                &[expected_m_offsets[partition_idx], 0]
            );

            assert_eq!(b_view.shape().get_dims().as_slice(), &[5, 12]);
            assert_eq!(b_view.offsets().get_dims().as_slice(), &[0, 0]);

            assert_eq!(
                out_view.shape().get_dims().as_slice(),
                &[expected_m_lengths[partition_idx], 12]
            );
            assert_eq!(
                out_view.offsets().get_dims().as_slice(),
                &[expected_m_offsets[partition_idx], 0]
            );
        }
    }

    #[test]
    fn can_partition_m_and_n_when_needed() {
        let operator = OperatorGemm {};
        let inputs = vec![tensor(&[4, 5]), tensor(&[5, 6])];
        let outputs = vec![tensor(&[4, 6])];

        let partitions = partition_tensors(&operator, &inputs, &outputs, 8).unwrap();
        assert_eq!(partitions.len(), 8);

        let expected: &[PartitionOffsetsShapes] = &[
            ((&[0, 0], &[1, 5]), (&[0, 0], &[5, 3]), (&[0, 0], &[1, 3])),
            ((&[0, 0], &[1, 5]), (&[0, 3], &[5, 3]), (&[0, 3], &[1, 3])),
            ((&[1, 0], &[1, 5]), (&[0, 0], &[5, 3]), (&[1, 0], &[1, 3])),
            ((&[1, 0], &[1, 5]), (&[0, 3], &[5, 3]), (&[1, 3], &[1, 3])),
            ((&[2, 0], &[1, 5]), (&[0, 0], &[5, 3]), (&[2, 0], &[1, 3])),
            ((&[2, 0], &[1, 5]), (&[0, 3], &[5, 3]), (&[2, 3], &[1, 3])),
            ((&[3, 0], &[1, 5]), (&[0, 0], &[5, 3]), (&[3, 0], &[1, 3])),
            ((&[3, 0], &[1, 5]), (&[0, 3], &[5, 3]), (&[3, 3], &[1, 3])),
        ];
        check_partitions(&partitions, expected);
    }

    #[test]
    fn partitions_preserve_subset_view_offsets() {
        let operator = OperatorGemm {};
        let input_a = Tensor::new(&[8, 5], &DataType::Bf16, 0);
        let input_b = Tensor::new(&[5, 9], &DataType::Bf16, 0);
        let output = Tensor::new(&[8, 9], &DataType::Bf16, 0);

        let input_views = vec![
            Some(TensorView::new(input_a, &[4, 5], &[2, 0])),
            Some(TensorView::new(input_b, &[5, 6], &[0, 3])),
        ];
        let output_views = vec![Some(TensorView::new(output, &[4, 6], &[2, 3]))];

        let partitions = operator
            .partition_views(&input_views, &output_views, 8)
            .unwrap();
        assert_eq!(partitions.len(), 8);

        let expected: &[PartitionOffsetsShapes] = &[
            ((&[2, 0], &[1, 5]), (&[0, 3], &[5, 3]), (&[2, 3], &[1, 3])),
            ((&[2, 0], &[1, 5]), (&[0, 6], &[5, 3]), (&[2, 6], &[1, 3])),
            ((&[3, 0], &[1, 5]), (&[0, 3], &[5, 3]), (&[3, 3], &[1, 3])),
            ((&[3, 0], &[1, 5]), (&[0, 6], &[5, 3]), (&[3, 6], &[1, 3])),
            ((&[4, 0], &[1, 5]), (&[0, 3], &[5, 3]), (&[4, 3], &[1, 3])),
            ((&[4, 0], &[1, 5]), (&[0, 6], &[5, 3]), (&[4, 6], &[1, 3])),
            ((&[5, 0], &[1, 5]), (&[0, 3], &[5, 3]), (&[5, 3], &[1, 3])),
            ((&[5, 0], &[1, 5]), (&[0, 6], &[5, 3]), (&[5, 6], &[1, 3])),
        ];
        check_partitions(&partitions, expected);
    }
}
