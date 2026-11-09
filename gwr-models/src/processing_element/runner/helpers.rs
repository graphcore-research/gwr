// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use gwr_engine::types::SimError;

use crate::processing_element::program;
use crate::processing_element::program::syntax::{ExprSpec, IndexSpec};
use crate::processing_element::program::validate::ValidatedRegion;
use crate::processing_element::tensors::{HasShape, TensorView};

pub(super) fn align8(bytes: usize) -> Result<usize, SimError> {
    bytes
        .checked_add(7)
        .map(|value| value & !7)
        .ok_or_else(|| SimError("SRAM slot alignment overflows".into()))
}

pub(super) fn assignment_dependencies(
    region: &ValidatedRegion,
    allocation: &program::plan::LocalAllocation,
) -> Result<BTreeMap<usize, BTreeSet<usize>>, SimError> {
    let mut producers = BTreeMap::new();
    for (index, assignment) in region.program.assignments.iter().enumerate() {
        if let Some(value_output) = &assignment.output {
            producers.insert(value_output.as_str(), index);
        }
        if let Some(index_output) = &assignment.indices_output {
            producers.insert(index_output.as_str(), index);
        }
    }
    let mut dependencies = BTreeMap::new();
    let mut consumers = BTreeMap::<String, BTreeSet<usize>>::new();
    for (index, assignment) in region.program.assignments.iter().enumerate() {
        let mut reads = BTreeSet::new();
        collect_read_names(&assignment.expr, &mut reads);
        let mut required = BTreeSet::new();
        for name in reads {
            if let Some(producer) = producers.get(name.as_str()) {
                required.insert(*producer);
                consumers.entry(name).or_default().insert(index);
            }
        }
        dependencies.insert(index, required);
    }
    for slot in &allocation.slots {
        for pair in slot.values.windows(2) {
            let next = *producers
                .get(pair[1].as_str())
                .ok_or_else(|| SimError("local slot has no producer".into()))?;
            let prior = *producers
                .get(pair[0].as_str())
                .ok_or_else(|| SimError("local slot has no producer".into()))?;
            dependencies.get_mut(&next).unwrap().insert(prior);
            if let Some(uses) = consumers.get(pair[0].as_str()) {
                dependencies.get_mut(&next).unwrap().extend(uses);
            }
        }
    }
    Ok(dependencies)
}

pub(super) fn collect_read_names(expr: &ExprSpec, reads: &mut BTreeSet<String>) {
    match expr {
        ExprSpec::Read { tensor, .. } => {
            reads.insert(tensor.clone());
        }
        ExprSpec::Call { args, .. } | ExprSpec::MatrixMac { args, .. } => {
            for arg in args {
                collect_read_names(arg, reads);
            }
        }
        ExprSpec::Reduce { value, .. } => {
            collect_read_names(value, reads);
        }
    }
}

pub(super) fn find_read<'a>(expr: &'a ExprSpec, name: &str) -> Option<&'a [IndexSpec]> {
    match expr {
        ExprSpec::Read {
            tensor, indices, ..
        } if tensor == name => Some(indices),
        ExprSpec::Call { args, .. } | ExprSpec::MatrixMac { args, .. } => {
            args.iter().find_map(|arg| find_read(arg, name))
        }
        ExprSpec::Reduce { value, .. } => find_read(value, name),
        ExprSpec::Read { .. } => None,
    }
}

pub(super) fn slice_axes(
    view: &TensorView,
    axes: &[String],
    intervals: &std::collections::BTreeMap<String, Range<usize>>,
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
    intervals: &std::collections::BTreeMap<String, Range<usize>>,
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
    coords: &std::collections::BTreeMap<String, usize>,
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
pub(super) fn expression_work(
    expr: &ExprSpec,
    region: &ValidatedRegion,
    result_dtype: crate::processing_element::tensors::dtype::DataType,
    count: usize,
) -> Result<
    (
        crate::processing_element::compute::ComputeWork,
        crate::processing_element::tensors::dtype::DataType,
    ),
    SimError,
> {
    use crate::processing_element::compute::{ComputeWork, TypedWork, WorkStage};
    match expr {
        ExprSpec::Read { tensor, .. } => Ok((
            ComputeWork { stages: vec![] },
            region.value(tensor).unwrap().dtype,
        )),
        ExprSpec::Call { op, args, .. } => {
            let mut stages = Vec::new();
            let mut inputs = Vec::new();
            for arg in args {
                let (work, dtype) = expression_work(arg, region, result_dtype, count)?;
                stages.extend(work.stages);
                inputs.push(dtype);
            }
            stages.push(WorkStage {
                operations: vec![TypedWork::operation(op, inputs, count)],
            });
            Ok((ComputeWork { stages }, result_dtype))
        }
        _ => Err(SimError(
            "nested reduction or matrix expression is unsupported".into(),
        )),
    }
}

pub(super) fn expression_read_dtype(
    expr: &ExprSpec,
    region: &ValidatedRegion,
) -> crate::processing_element::tensors::dtype::DataType {
    match expr {
        ExprSpec::Read { tensor, .. } => region.value(tensor).unwrap().dtype,
        ExprSpec::Call { args, .. } => expression_read_dtype(&args[0], region),
        _ => unreachable!("validated scalar expression"),
    }
}

#[cfg(test)]
mod expression_dtype_contract {
    use super::expression_work;
    use crate::processing_element::compute::{ComputeConfig, ComputeModel, ComputeWork};
    use crate::processing_element::program::syntax::ProgramSpec;
    use crate::processing_element::program::validate::{RegionBindings, validate_program};
    use crate::processing_element::tensors::dtype::DataType as D;
    use crate::processing_element::tensors::{Tensor, TensorView};

    fn works(source: &str, input: D, output: D) -> Vec<ComputeWork> {
        let spec: ProgramSpec = yaml_serde::from_str(source).unwrap();
        let view =
            |dtype, address| TensorView::new_full(Tensor::new(&[4], &dtype, address).unwrap());
        let region = validate_program(
            &spec,
            &RegionBindings {
                inputs: vec![view(input, 0)],
                outputs: vec![view(output, 128)],
            },
        )
        .unwrap();
        spec.assignments
            .iter()
            .map(|assignment| {
                let dtype = region.value(&assignment.primary_result()).unwrap().dtype;
                let (work, result) = expression_work(&assignment.expr, &region, dtype, 4).unwrap();
                assert_eq!(result, dtype);
                work
            })
            .collect()
    }

    fn signatures(source: &str, input: D, output: D) -> Vec<Vec<(String, Vec<D>, usize)>> {
        works(source, input, output)
            .into_iter()
            .map(|work| {
                work.stages
                    .into_iter()
                    .flat_map(|stage| stage.operations)
                    .map(|operation| {
                        (
                            operation.signature.op,
                            operation.signature.inputs,
                            operation.count,
                        )
                    })
                    .collect()
            })
            .collect()
    }

    const NESTED: &str = "{ mul: [{ mul: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: A, indices: [i] } }] }, { read: { tensor: A, indices: [i] } }] }";

    #[test]
    fn nested_calls_return_assignment_dtype_without_extra_cast_work() {
        let source = format!(
            "inputs: [A]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: {NESTED}\n"
        );
        assert_eq!(
            signatures(&source, D::Fp32, D::Bf16),
            vec![vec![
                ("mul".into(), vec![D::Fp32, D::Fp32], 4),
                ("mul".into(), vec![D::Bf16, D::Fp32], 4),
            ]]
        );
    }

    #[test]
    fn enclosing_convert_does_not_establish_a_child_dtype_scope() {
        let source = format!(
            "inputs: [A]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: {{ convert: [{NESTED}] }}\n"
        );
        assert_eq!(
            signatures(&source, D::Fp32, D::Bf16),
            vec![vec![
                ("mul".into(), vec![D::Fp32, D::Fp32], 4),
                ("mul".into(), vec![D::Bf16, D::Fp32], 4),
                ("convert".into(), vec![D::Bf16], 4),
            ]]
        );
    }

    #[test]
    fn explicit_fp32_assignment_then_convert_preserves_opmath_signatures() {
        let source = format!(
            "inputs: [A]\noutputs: [Y]\ntemporaries: {{ Mixed32: {{ dtype: fp32 }} }}\nassignments:\n  - output: Mixed32\n    axes: [i]\n    expr: {NESTED}\n  - output: Y\n    axes: [i]\n    expr: {{ convert: [{{ read: {{ tensor: Mixed32, indices: [i] }} }}] }}\n"
        );
        assert_eq!(
            signatures(&source, D::Fp32, D::Bf16),
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
    fn actual_helper_work_selects_distinct_typed_services_and_explicit_conversion_cost() {
        let classes = yaml_serde::from_str(
            "mul: { operations: [mul], arity: 2 }\ncast: { operations: [convert], arity: 1 }",
        )
        .unwrap();
        let config: ComputeConfig = yaml_serde::from_str(
            "engines:\n  vector:\n    capabilities:\n      - { class: mul, input_dtypes: [[fp32, fp32]], ops_per_tick: 1 }\n      - { class: mul, input_dtypes: [[bf16, fp32]], ops_per_tick: 4 }\n      - { class: cast, input_dtypes: [[fp32]], ops_per_tick: 2 }"
        ).unwrap();
        let model = ComputeModel::new(&config, &classes).unwrap();
        let legacy = format!(
            "inputs: [A]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: {NESTED}\n"
        );
        let explicit = format!(
            "inputs: [A]\noutputs: [Y]\ntemporaries: {{ Mixed32: {{ dtype: fp32 }} }}\nassignments:\n  - output: Mixed32\n    axes: [i]\n    expr: {NESTED}\n  - output: Y\n    axes: [i]\n    expr: {{ convert: [{{ read: {{ tensor: Mixed32, indices: [i] }} }}] }}\n"
        );
        for (source, ticks) in [(legacy, 5), (explicit, 10)] {
            let service: u64 = works(&source, D::Fp32, D::Bf16)
                .iter()
                .map(|work| model.lower(work).unwrap().service_ticks().unwrap())
                .sum();
            assert_eq!(service, ticks);
        }
    }
}
