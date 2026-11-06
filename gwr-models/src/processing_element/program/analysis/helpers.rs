// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::ops::Range;

use gwr_engine::types::SimError;

use crate::processing_element::compute::{ComputeWork, TypedWork, WorkStage};
use crate::processing_element::program::executable::{Read, ScalarExpr};
use crate::processing_element::program::syntax::IndexSpec;
use crate::processing_element::program::validate::ProgramFacts;
use crate::processing_element::tensors::dtype::DataType;
use crate::processing_element::tensors::{HasShape, TensorView};

pub(super) fn slice_axes(
    view: &TensorView,
    axes: &[String],
    intervals: &BTreeMap<String, Range<usize>>,
) -> Result<TensorView, SimError> {
    let indices = axes
        .iter()
        .cloned()
        .map(IndexSpec::Axis)
        .collect::<Vec<_>>();
    slice_view(view, &indices, intervals)
}

pub(super) fn slice_view(
    view: &TensorView,
    indices: &[IndexSpec],
    intervals: &BTreeMap<String, Range<usize>>,
) -> Result<TensorView, SimError> {
    let mut shape = view.shape().dims().to_vec();
    let mut offsets = view.offsets().to_vec();
    for (dimension, index) in indices.iter().enumerate() {
        if shape[dimension] == 1 {
            continue;
        }
        if let IndexSpec::Axis(axis) = index
            && let Some(range) = intervals.get(axis)
        {
            offsets[dimension] = offsets[dimension]
                .checked_add(range.start)
                .ok_or_else(|| SimError("tile view offset overflows".into()))?;
            shape[dimension] = range.len();
        }
    }
    TensorView::new(view.tensor().clone(), &shape, &offsets)
}

pub(super) fn eval_index(
    index: &IndexSpec,
    coords: &BTreeMap<String, usize>,
) -> Result<i128, SimError> {
    match index {
        IndexSpec::Zero(value) => {
            i128::try_from(*value).map_err(|_| SimError("window index overflows".into()))
        }
        IndexSpec::Axis(axis) => {
            i128::try_from(coords[axis]).map_err(|_| SimError("window index overflows".into()))
        }
        IndexSpec::Affine { affine } => {
            affine
                .terms
                .iter()
                .try_fold(i128::from(affine.offset), |sum, (axis, coefficient)| {
                    let position = i128::try_from(coords[axis])
                        .map_err(|_| SimError("window index overflows".into()))?;
                    let coefficient = i128::try_from(*coefficient)
                        .map_err(|_| SimError("window coefficient overflows".into()))?;
                    sum.checked_add(
                        position
                            .checked_mul(coefficient)
                            .ok_or_else(|| SimError("window index overflows".into()))?,
                    )
                    .ok_or_else(|| SimError("window index overflows".into()))
                })
        }
    }
}

/// Lower scalar expression trees with barriers between dependencies. Nested
/// results use the assignment dtype and explicit local chunk storage. Reads
/// keep their declared dtype; FP32 operands do not promote call results, and
/// enclosing casts do not introduce a child dtype scope. Only explicit
/// conversion calls add conversion work. Preserve higher-precision opmath
/// through a typed temporary assignment followed by a separate explicit
/// conversion assignment.
pub(super) fn build_scalar_expression_work(
    expr: &ScalarExpr,
    region: &ProgramFacts,
    result_dtype: DataType,
    count: usize,
) -> Result<(ComputeWork, DataType), SimError> {
    match expr {
        ScalarExpr::Read(Read { tensor, .. }) => Ok((
            ComputeWork { stages: vec![] },
            region.value(tensor).unwrap().dtype,
        )),
        ScalarExpr::Call { op, args, .. } => {
            let mut stages = Vec::new();
            let mut inputs = Vec::new();
            for arg in args {
                let (work, dtype) = build_scalar_expression_work(arg, region, result_dtype, count)?;
                stages.extend(work.stages);
                inputs.push(dtype);
            }
            stages.push(WorkStage {
                operations: vec![TypedWork::operation(op, inputs, count)],
            });
            Ok((ComputeWork { stages }, result_dtype))
        }
    }
}

pub(super) fn find_first_read_dtype(expr: &ScalarExpr, region: &ProgramFacts) -> DataType {
    match expr {
        ScalarExpr::Read(Read { tensor, .. }) => region.value(tensor).unwrap().dtype,
        ScalarExpr::Call { args, .. } => find_first_read_dtype(&args[0], region),
    }
}

#[cfg(test)]
mod expression_dtype_contract {
    use yaml_serde::from_str;

    use super::build_scalar_expression_work;
    use crate::processing_element::compute::{ComputeConfig, ComputeModel, ComputeWork};
    use crate::processing_element::program::syntax::ProgramSpec;
    use crate::processing_element::program::validate::{RegionBindings, validate_program};
    use crate::processing_element::tensors::dtype::DataType as D;
    use crate::processing_element::tensors::{Tensor, TensorView};

    fn assignment_operation_signatures(
        source: &str,
        input: D,
        output: D,
    ) -> Vec<Vec<(String, Vec<D>, usize)>> {
        lower_assignment_expressions(source, input, output)
            .into_iter()
            .map(|work| {
                work.stages
                    .into_iter()
                    .flat_map(|stage| stage.operations)
                    .map(|operation| {
                        (
                            operation.op().to_owned(),
                            operation.inputs().to_vec(),
                            operation.count(),
                        )
                    })
                    .collect()
            })
            .collect()
    }

    fn lower_assignment_expressions(source: &str, input: D, output: D) -> Vec<ComputeWork> {
        let spec: ProgramSpec = from_str(source).unwrap();
        let view =
            |dtype, address| TensorView::new_full(Tensor::new(&[4], &dtype, address).unwrap());
        let region = validate_program(
            &spec,
            RegionBindings {
                inputs: vec![view(input, 0)],
                outputs: vec![view(output, 128)],
            },
        )
        .unwrap();
        region
            .program
            .assignments
            .iter()
            .map(|assignment| {
                let dtype = region.value(assignment.primary_result()).unwrap().dtype;
                let (work, result) = build_scalar_expression_work(
                    assignment.expr.scalar().unwrap(),
                    &region,
                    dtype,
                    4,
                )
                .unwrap();
                assert_eq!(result, dtype);
                work
            })
            .collect()
    }

    const NESTED: &str = r"
mul:
  - mul:
      - read: { tensor: A, indices: [i] }
      - read: { tensor: A, indices: [i] }
  - read: { tensor: A, indices: [i] }
";

    #[test]
    fn nested_calls_return_assignment_dtype_without_extra_cast_work() {
        let nested = NESTED.trim().replace('\n', "\n      ");
        let source = format!(
            r"
inputs: [A]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      {nested}
"
        );
        assert_eq!(
            assignment_operation_signatures(&source, D::Fp32, D::Bf16),
            vec![vec![
                ("mul".into(), vec![D::Fp32, D::Fp32], 4),
                ("mul".into(), vec![D::Bf16, D::Fp32], 4),
            ]]
        );
    }

    #[test]
    fn enclosing_conversion_keeps_nested_calls_in_the_assignment_dtype() {
        let nested = NESTED.trim().replace('\n', "\n          ");
        let source = format!(
            r"
inputs: [A]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      convert:
        - {nested}
"
        );
        assert_eq!(
            assignment_operation_signatures(&source, D::Fp32, D::Bf16),
            vec![vec![
                ("mul".into(), vec![D::Fp32, D::Fp32], 4),
                ("mul".into(), vec![D::Bf16, D::Fp32], 4),
                ("convert".into(), vec![D::Bf16], 4),
            ]]
        );
    }

    #[test]
    fn fp32_temporary_keeps_nested_multiply_inputs_fp32_before_conversion() {
        let nested = NESTED.trim().replace('\n', "\n      ");
        let source = format!(
            r"
inputs: [A]
outputs: [Y]
temporaries: {{ Mixed32: {{ dtype: fp32 }} }}
assignments:
  - output: Mixed32
    axes: [i]
    expr:
      {nested}
  - output: Y
    axes: [i]
    expr:
      convert:
        - read: {{ tensor: Mixed32, indices: [i] }}
"
        );
        assert_eq!(
            assignment_operation_signatures(&source, D::Fp32, D::Bf16),
            vec![
                vec![
                    ("mul".into(), vec![D::Fp32, D::Fp32], 4),
                    ("mul".into(), vec![D::Fp32, D::Fp32], 4),
                ],
                vec![("convert".into(), vec![D::Fp32], 4)]
            ]
        );
    }

    #[test]
    fn typed_expression_service_cost_includes_explicit_conversion() {
        let nested = NESTED.trim().replace('\n', "\n      ");
        let classes = from_str(
            "mul: { operations: [mul], arity: 2 }\ncast: { operations: [convert], arity: 1 }",
        )
        .unwrap();
        let config: ComputeConfig = from_str(
            "execution_units:\n  vector:\n    capabilities:\n      - { class: mul, input_dtypes: [[fp32, fp32]], ops_per_tick: 1 }\n      - { class: mul, input_dtypes: [[bf16, fp32]], ops_per_tick: 4 }\n      - { class: cast, input_dtypes: [[fp32]], ops_per_tick: 2 }"
        ).unwrap();
        let model = ComputeModel::new(&config, &classes).unwrap();
        let direct_output = format!(
            r"
inputs: [A]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      {nested}
"
        );
        let explicit_conversion = format!(
            r"
inputs: [A]
outputs: [Y]
temporaries: {{ Mixed32: {{ dtype: fp32 }} }}
assignments:
  - output: Mixed32
    axes: [i]
    expr:
      {nested}
  - output: Y
    axes: [i]
    expr:
      convert:
        - read: {{ tensor: Mixed32, indices: [i] }}
"
        );
        for (source, ticks) in [(direct_output, 5), (explicit_conversion, 10)] {
            let service: u64 = lower_assignment_expressions(&source, D::Fp32, D::Bf16)
                .iter()
                .map(|work| model.lower(work).unwrap().service_ticks().unwrap())
                .sum();
            assert_eq!(service, ticks);
        }
    }
}
