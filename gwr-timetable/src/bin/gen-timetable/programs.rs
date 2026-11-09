// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Program constructors used by the synthetic timetable generator.

use std::collections::BTreeMap;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;
use gwr_models::processing_element::program::syntax::{
    AssignmentSpec, BoundsMode, ExprSpec, IndexSpec, ProgramSpec, ReductionOp, TemporarySpec,
};
use gwr_models::processing_element::program::validate::TensorSpec;

pub(super) fn reduce_max_program(
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
        metadata_inputs: vec![],
        indexed_metadata_inputs: vec![],
        local_round_policy: None,
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
                value: Box::new(read("A", indices)),
            },
        }],
    })
}

pub(super) fn add_program(
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
        metadata_inputs: vec![],
        indexed_metadata_inputs: vec![],
        local_round_policy: None,
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
                    read("A", operand_indices.remove(0)),
                    read("B", operand_indices.remove(0)),
                ],
                attrs: BTreeMap::new(),
            },
        }],
    })
}

#[expect(clippy::too_many_lines)]
pub(super) fn gemm_program(
    inputs: &[TensorSpec],
    output: &TensorSpec,
) -> Result<ProgramSpec, SimError> {
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
    let mut batch_axes = Vec::new();
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
        batch_axes.push(format!("b{index}"));
    }
    if inputs.len() == 3 && inputs[2].shape != [n] && inputs[2].shape != output.shape {
        return sim_error!("matrix bias must have shape [n] or the output shape");
    }
    let m_axis = IndexSpec::Axis("m".into());
    let n_axis = IndexSpec::Axis("n".into());
    let k_axis = IndexSpec::Axis("k".into());
    let batch_indices = |rank: usize| {
        batch_axes[batch_rank - (rank - 2)..]
            .iter()
            .cloned()
            .map(IndexSpec::Axis)
            .collect::<Vec<_>>()
    };
    let mut lhs_indices = batch_indices(lhs_rank);
    lhs_indices.extend([m_axis.clone(), k_axis.clone()]);
    let mut rhs_indices = batch_indices(rhs_rank);
    rhs_indices.extend([k_axis.clone(), n_axis.clone()]);
    let product = ExprSpec::MatrixMac {
        execution: None,
        quantization: None,
        args: vec![read("A", lhs_indices), read("B", rhs_indices)],
        over: vec!["k".into()],
    };
    let axes = batch_axes
        .into_iter()
        .chain(["m".into(), "n".into()])
        .collect::<Vec<_>>();
    let mut program = ProgramSpec {
        metadata_inputs: vec![],
        indexed_metadata_inputs: vec![],
        local_round_policy: None,
        inputs: vec!["A".into(), "B".into()],
        outputs: vec!["Y".into()],
        temporaries: BTreeMap::new(),
        extents: BTreeMap::new(),
        assignments: Vec::new(),
    };
    if inputs.len() == 3 {
        program.inputs.push("bias".into());
        program.temporaries.insert(
            "product".into(),
            TemporarySpec {
                dtype: output.dtype,
            },
        );
        program.assignments.push(AssignmentSpec {
            output: Some("product".into()),
            indices_output: None,
            axes: axes.clone(),
            expr: product,
        });
        let bias_indices = if inputs[2].shape.len() == 1 {
            vec![n_axis]
        } else {
            axes.iter().cloned().map(IndexSpec::Axis).collect()
        };
        program.assignments.push(AssignmentSpec {
            output: Some("Y".into()),
            indices_output: None,
            axes: axes.clone(),
            expr: ExprSpec::Call {
                op: "add".into(),
                args: vec![
                    read(
                        "product",
                        axes.iter().cloned().map(IndexSpec::Axis).collect(),
                    ),
                    read("bias", bias_indices),
                ],
                attrs: BTreeMap::new(),
            },
        });
    } else {
        program.assignments.push(AssignmentSpec {
            output: Some("Y".into()),
            indices_output: None,
            axes,
            expr: product,
        });
    }
    Ok(program)
}

fn read(tensor: &str, indices: Vec<IndexSpec>) -> ExprSpec {
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

    #[test]
    fn reduce_max_has_optional_indices_output() {
        let input = TensorSpec {
            shape: vec![2, 3, 4],
            dtype: DataType::Fp32,
        };
        let values_only = reduce_max_program(&input, false).unwrap();
        let with_indices = reduce_max_program(&input, true).unwrap();
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
    fn constructors_emit_valid_single_path_programs() {
        let add = add_program(
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

        let gemm = gemm_program(
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
    fn batched_matrix_constructor_handles_broadcast_and_bias() {
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
        let program = gemm_program(&inputs, &output).unwrap();
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

    fn view(shape: &[usize], dtype: DataType, addr: u64) -> TensorView {
        TensorView::new_full(Tensor::new(shape, &dtype, addr).unwrap())
    }
}
