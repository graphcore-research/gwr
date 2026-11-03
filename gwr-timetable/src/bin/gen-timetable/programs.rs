// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Program constructors used by the synthetic timetable generator.

use std::collections::BTreeMap;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;
use gwr_models::processing_element::program::syntax::{
    AssignmentSpec, BoundsMode, ExprSpec, IndexSpec, ProgramSpec, ReductionOp, TemporarySpec,
};
use gwr_models::processing_element::program::validate::TensorSpec;

pub(super) fn build_reduce_max_program(
    input: &TensorSpec,
    include_indices: bool,
) -> Result<ProgramSpec, SimError> {
    if input.shape.len() < 2 {
        return sim_error!("max reduction requires rank at least two");
    }
    let axes = (0..input.shape.len() - 1)
        .map(|index| format!("d{index}"))
        .collect::<Vec<_>>();
    let indices = axes
        .iter()
        .cloned()
        .chain(["k".into()])
        .map(IndexSpec::Axis)
        .collect();
    Ok(ProgramSpec {
        inputs: vec!["A".into()],
        outputs: if include_indices {
            vec!["Y".into(), "I".into()]
        } else {
            vec!["Y".into()]
        },
        temporaries: BTreeMap::new(),
        extents: BTreeMap::new(),
        assignments: vec![AssignmentSpec {
            output: Some("Y".into()),
            indices_output: include_indices.then(|| "I".into()),
            axes,
            expr: ExprSpec::Reduce {
                op: ReductionOp::Max,
                over: vec!["k".into()],
                value: Box::new(build_tensor_read_expression("A", indices)),
            },
        }],
    })
}

pub(super) fn build_add_program(
    inputs: &[TensorSpec],
    output: &TensorSpec,
) -> Result<ProgramSpec, SimError> {
    if inputs.len() != 2 {
        return sim_error!("add constructor requires two inputs");
    }
    let rank = output.shape.len();
    let axes = (0..rank).map(|i| format!("d{i}")).collect::<Vec<_>>();
    let mut operand_indices = Vec::new();
    for input in inputs {
        if input.shape.len() > rank {
            return sim_error!("add input rank exceeds output rank");
        }
        let aligned = &axes[rank - input.shape.len()..];
        for (dimension, axis) in input.shape.iter().zip(aligned) {
            if *dimension != 1
                && *dimension
                    != output.shape[axes.iter().position(|candidate| candidate == axis).unwrap()]
            {
                return sim_error!("add broadcast shape mismatch");
            }
        }
        operand_indices.push(
            aligned
                .iter()
                .cloned()
                .map(IndexSpec::Axis)
                .collect::<Vec<_>>(),
        );
    }
    Ok(ProgramSpec {
        inputs: vec!["A".into(), "B".into()],
        outputs: vec!["Y".into()],
        temporaries: BTreeMap::new(),
        extents: BTreeMap::new(),
        assignments: vec![AssignmentSpec {
            output: Some("Y".into()),
            indices_output: None,
            axes: axes.clone(),
            expr: ExprSpec::Call {
                op: "add".into(),
                args: vec![
                    build_tensor_read_expression("A", operand_indices.remove(0)),
                    build_tensor_read_expression("B", operand_indices.remove(0)),
                ],
            },
        }],
    })
}

pub(super) fn build_gemm_program(
    inputs: &[TensorSpec],
    output: &TensorSpec,
) -> Result<ProgramSpec, SimError> {
    let batch_rank = validate_gemm_shapes(inputs, output)?;
    let batch_axes = (0..batch_rank)
        .map(|index| format!("b{index}"))
        .collect::<Vec<_>>();
    let product = build_matrix_product_expression(inputs, output, &batch_axes);
    let axes = batch_axes
        .into_iter()
        .chain(["m".into(), "n".into()])
        .collect();
    let mut program = ProgramSpec {
        inputs: vec!["A".into(), "B".into()],
        outputs: vec!["Y".into()],
        temporaries: BTreeMap::new(),
        extents: BTreeMap::new(),
        assignments: vec![AssignmentSpec {
            output: Some("Y".into()),
            indices_output: None,
            axes,
            expr: product,
        }],
    };
    if let Some(bias) = inputs.get(2) {
        add_matrix_bias(&mut program, bias, output);
    }
    Ok(program)
}

fn validate_gemm_shapes(inputs: &[TensorSpec], output: &TensorSpec) -> Result<usize, SimError> {
    if !(2..=3).contains(&inputs.len())
        || inputs[0].shape.len() < 2
        || inputs[1].shape.len() < 2
        || output.shape.len() < 2
    {
        return sim_error!(
            "matrix constructor requires two rank-at-least-2 inputs, an optional bias, and a rank-at-least-2 output"
        );
    }
    let lhs_rank = inputs[0].shape.len();
    let rhs_rank = inputs[1].shape.len();
    let batch_rank = (lhs_rank - 2).max(rhs_rank - 2);
    if output.shape.len() != batch_rank + 2 {
        return sim_error!("matrix output batch rank mismatch");
    }
    let (m, k) = (inputs[0].shape[lhs_rank - 2], inputs[0].shape[lhs_rank - 1]);
    let (rhs_k, n) = (inputs[1].shape[rhs_rank - 2], inputs[1].shape[rhs_rank - 1]);
    if k != rhs_k || output.shape[batch_rank..] != [m, n] {
        return sim_error!("matrix constructor shape mismatch");
    }
    for index in 0..batch_rank {
        let lhs_size = if index >= batch_rank - (lhs_rank - 2) {
            inputs[0].shape[index - (batch_rank - (lhs_rank - 2))]
        } else {
            1
        };
        let rhs_size = if index >= batch_rank - (rhs_rank - 2) {
            inputs[1].shape[index - (batch_rank - (rhs_rank - 2))]
        } else {
            1
        };
        if lhs_size != rhs_size && lhs_size != 1 && rhs_size != 1 {
            return sim_error!("matrix batch dimensions are incompatible");
        }
        if output.shape[index] != lhs_size.max(rhs_size) {
            return sim_error!("matrix output batch shape mismatch");
        }
    }
    if inputs.len() == 3 && inputs[2].shape != [n] && inputs[2].shape != output.shape {
        return sim_error!("matrix bias must have shape [n] or the output shape");
    }
    Ok(batch_rank)
}

fn build_matrix_product_expression(
    inputs: &[TensorSpec],
    output: &TensorSpec,
    batch_axes: &[String],
) -> ExprSpec {
    let m_axis = IndexSpec::Axis("m".into());
    let n_axis = IndexSpec::Axis("n".into());
    let k_axis = IndexSpec::Axis("k".into());
    let batch_rank = batch_axes.len();
    let batch_indices = |rank: usize| {
        batch_axes[batch_rank - (rank - 2)..]
            .iter()
            .cloned()
            .map(IndexSpec::Axis)
            .collect::<Vec<_>>()
    };
    let mut lhs_indices = batch_indices(inputs[0].shape.len());
    lhs_indices.extend([m_axis, k_axis.clone()]);
    let mut rhs_indices = batch_indices(inputs[1].shape.len());
    rhs_indices.extend([k_axis, n_axis]);
    ExprSpec::MatrixMac {
        accumulator: output.dtype,
        args: vec![
            build_tensor_read_expression("A", lhs_indices),
            build_tensor_read_expression("B", rhs_indices),
        ],
        over: vec!["k".into()],
    }
}

fn add_matrix_bias(program: &mut ProgramSpec, bias: &TensorSpec, output: &TensorSpec) {
    let product = &mut program.assignments[0];
    product.output = Some("product".into());
    let axes = product.axes.clone();
    let product_indices = axes
        .iter()
        .cloned()
        .map(IndexSpec::Axis)
        .collect::<Vec<_>>();
    let bias_indices = if bias.shape.len() == 1 {
        vec![IndexSpec::Axis("n".into())]
    } else {
        product_indices.clone()
    };
    program.inputs.push("bias".into());
    program.temporaries.insert(
        "product".into(),
        TemporarySpec {
            dtype: output.dtype,
        },
    );
    program.assignments.push(AssignmentSpec {
        output: Some("Y".into()),
        indices_output: None,
        axes,
        expr: ExprSpec::Call {
            op: "add".into(),
            args: vec![
                build_tensor_read_expression("product", product_indices),
                build_tensor_read_expression("bias", bias_indices),
            ],
        },
    });
}

fn build_tensor_read_expression(tensor: &str, indices: Vec<IndexSpec>) -> ExprSpec {
    ExprSpec::Read {
        tensor: tensor.into(),
        indices,
        bounds: BoundsMode::RequireInRange,
    }
}

#[cfg(test)]
mod tests {
    use gwr_models::processing_element::program::validate::{RegionBindings, validate_program};
    use gwr_models::processing_element::tensors::dtype::DataType;
    use gwr_models::processing_element::tensors::{Tensor, TensorView};

    use super::*;

    fn view(shape: &[usize], dtype: DataType, addr: u64) -> TensorView {
        TensorView::new_full(Tensor::new(shape, &dtype, addr).unwrap())
    }

    #[test]
    fn reduce_max_has_optional_indices_output() {
        let input = TensorSpec {
            shape: vec![2, 3, 4],
            dtype: DataType::Fp32,
        };
        let values_only = build_reduce_max_program(&input, false).unwrap();
        let with_indices = build_reduce_max_program(&input, true).unwrap();
        assert_eq!(values_only.outputs, ["Y"]);
        assert_eq!(with_indices.outputs, ["Y", "I"]);
        assert_eq!(
            with_indices.assignments[0].indices_output.as_deref(),
            Some("I")
        );
        let region = validate_program(
            &with_indices,
            &RegionBindings {
                inputs: vec![view(&[2, 3, 4], DataType::Fp32, 0)],
                outputs: vec![
                    view(&[2, 3], DataType::Fp32, 128),
                    view(&[2, 3], DataType::Int64, 256),
                ],
            },
        )
        .unwrap();
        assert_eq!(region.extent("k"), Some(4));
        assert_eq!(region.value("Y").unwrap().shape, [2, 3]);
    }

    #[test]
    fn add_constructor_builds_a_valid_elementwise_program() {
        let add = build_add_program(
            &[
                TensorSpec {
                    shape: vec![3, 4],
                    dtype: DataType::Fp32,
                },
                TensorSpec {
                    shape: vec![3, 4],
                    dtype: DataType::Fp32,
                },
            ],
            &TensorSpec {
                shape: vec![3, 4],
                dtype: DataType::Fp32,
            },
        )
        .unwrap();
        validate_program(
            &add,
            &RegionBindings {
                inputs: vec![
                    view(&[3, 4], DataType::Fp32, 0),
                    view(&[3, 4], DataType::Fp32, 100),
                ],
                outputs: vec![view(&[3, 4], DataType::Fp32, 200)],
            },
        )
        .unwrap();
    }

    #[test]
    fn gemm_constructor_infers_output_shape_and_reduction_extent() {
        let gemm = build_gemm_program(
            &[
                TensorSpec {
                    shape: vec![2, 4],
                    dtype: DataType::Fp16,
                },
                TensorSpec {
                    shape: vec![4, 3],
                    dtype: DataType::Fp16,
                },
            ],
            &TensorSpec {
                shape: vec![2, 3],
                dtype: DataType::Fp32,
            },
        )
        .unwrap();
        let region = validate_program(
            &gemm,
            &RegionBindings {
                inputs: vec![
                    view(&[2, 4], DataType::Fp16, 300),
                    view(&[4, 3], DataType::Fp16, 400),
                ],
                outputs: vec![view(&[2, 3], DataType::Fp32, 500)],
            },
        )
        .unwrap();
        assert_eq!(region.extent("k"), Some(4));
        assert_eq!(region.value("Y").unwrap().shape, [2, 3]);
    }

    #[test]
    fn validates_batched_matrix_program_with_broadcast_inputs_and_bias() {
        let inputs = [
            TensorSpec {
                shape: vec![2, 1, 3, 5],
                dtype: DataType::Fp16,
            },
            TensorSpec {
                shape: vec![1, 4, 5, 7],
                dtype: DataType::Bf16,
            },
            TensorSpec {
                shape: vec![7],
                dtype: DataType::Fp32,
            },
        ];
        let output = TensorSpec {
            shape: vec![2, 4, 3, 7],
            dtype: DataType::Fp32,
        };
        let program = build_gemm_program(&inputs, &output).unwrap();
        let region = validate_program(
            &program,
            &RegionBindings {
                inputs: vec![
                    view(&inputs[0].shape, inputs[0].dtype, 0),
                    view(&inputs[1].shape, inputs[1].dtype, 512),
                    view(&inputs[2].shape, inputs[2].dtype, 1024),
                ],
                outputs: vec![view(&output.shape, output.dtype, 2048)],
            },
        )
        .unwrap();
        assert_eq!(region.value("product").unwrap().shape, output.shape);
    }
}
