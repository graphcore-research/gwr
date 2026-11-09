// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Validation of tensor-program names, shapes, indices, and storage boundaries.

use std::collections::{BTreeMap, BTreeSet};

use gwr_engine::types::SimError;

use super::syntax::{AssignmentSpec, BoundsMode, ExprSpec, IndexSpec, ProgramSpec, ReductionOp};
use crate::processing_element::compute::is_lowercase_operation;
use crate::processing_element::tensors::dtype::DataType;
use crate::processing_element::tensors::{HasShape, TensorView};

pub fn validate_program(
    spec: &ProgramSpec,
    bindings: &RegionBindings,
) -> Result<ValidatedRegion, SimError> {
    validate_program_header(spec, bindings)?;
    let producers = assignment_producers(spec)?;
    let order = assignment_order(spec, &producers)?;
    let mut validation = AssignmentValidation::new(spec, bindings, &producers);
    for &index in &order {
        validation.validate(&spec.assignments[index])?;
    }
    super::indexed_metadata::validate(spec, bindings, &validation.extents)?;
    validate_boundary_overlap(spec, bindings)?;
    Ok(ValidatedRegion {
        program: spec.clone(),
        bindings: bindings.clone(),
        values: validation.values,
        extents: validation.extents,
        assignment_order: order,
    })
}

#[derive(Clone, Debug)]
pub struct ValidatedRegion {
    pub program: ProgramSpec,
    pub bindings: RegionBindings,
    pub values: BTreeMap<String, TensorSpec>,
    pub extents: BTreeMap<String, usize>,
    pub assignment_order: Vec<usize>,
}

impl ValidatedRegion {
    #[must_use]
    pub fn value(&self, name: &str) -> Option<&TensorSpec> {
        self.values.get(name)
    }

    #[must_use]
    pub fn extent(&self, axis: &str) -> Option<usize> {
        self.extents.get(axis).copied()
    }
}

fn validate_program_header(spec: &ProgramSpec, bindings: &RegionBindings) -> Result<(), SimError> {
    if spec
        .metadata_inputs
        .iter()
        .any(|name| !spec.inputs.contains(name))
        || spec.metadata_inputs.iter().collect::<BTreeSet<_>>().len() != spec.metadata_inputs.len()
    {
        return Err(err(
            "metadata_inputs must be a distinct subset of program inputs",
        ));
    }

    if spec.inputs.len() != bindings.inputs.len() {
        return Err(err(format!(
            "program input bindings differ: declared {}, bound {}",
            spec.inputs.len(),
            bindings.inputs.len()
        )));
    }
    if spec.outputs.len() != bindings.outputs.len() {
        return Err(err(format!(
            "program output bindings differ: declared {}, bound {}",
            spec.outputs.len(),
            bindings.outputs.len()
        )));
    }
    let mut names = BTreeSet::new();
    for name in spec.inputs.iter().chain(&spec.outputs) {
        register(&mut names, name)?;
    }
    for name in spec.temporaries.keys() {
        register(&mut names, name)?;
    }
    for (axis, extent) in &spec.extents {
        if axis.is_empty() || *extent == 0 {
            return Err(err(format!("axis {axis} has invalid extent {extent}")));
        }
    }
    Ok(())
}

fn assignment_producers(spec: &ProgramSpec) -> Result<BTreeMap<String, usize>, SimError> {
    let mut producers = BTreeMap::new();
    for (index, assignment) in spec.assignments.iter().enumerate() {
        if assignment.output.is_none() && assignment.indices_output.is_none() {
            return Err(err("assignment must name output or indices_output"));
        }
        if assignment.indices_output.is_some()
            && !matches!(
                assignment.expr,
                ExprSpec::Reduce {
                    op: ReductionOp::Min | ReductionOp::Max,
                    ..
                }
            )
        {
            return Err(err("indices_output requires a min or max reduction"));
        }
        for name in [
            assignment.output.as_ref(),
            assignment.indices_output.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if !spec.outputs.contains(name) && !spec.temporaries.contains_key(name) {
                return Err(err(format!("assignment targets undeclared value {name}")));
            }
            if producers.insert(name.clone(), index).is_some() {
                return Err(err(format!("duplicate producer for {name}")));
            }
        }
    }
    for name in spec.outputs.iter().chain(spec.temporaries.keys()) {
        if !producers.contains_key(name) {
            return Err(err(format!("missing producer for {name}")));
        }
    }
    Ok(producers)
}

fn assignment_order(
    spec: &ProgramSpec,
    producers: &BTreeMap<String, usize>,
) -> Result<Vec<usize>, SimError> {
    let mut order = Vec::new();
    let mut marks = vec![0; spec.assignments.len()];
    for index in 0..spec.assignments.len() {
        visit_assignment(index, spec, producers, &mut marks, &mut order)?;
    }
    Ok(order)
}

fn visit_assignment(
    index: usize,
    spec: &ProgramSpec,
    producers: &BTreeMap<String, usize>,
    marks: &mut [u8],
    order: &mut Vec<usize>,
) -> Result<(), SimError> {
    match marks[index] {
        2 => return Ok(()),
        1 => {
            return Err(err(format!(
                "cycle through assignment {}",
                spec.assignments[index].primary_result()
            )));
        }
        _ => {}
    }
    marks[index] = 1;
    let mut reads = BTreeSet::new();
    collect_reads(&spec.assignments[index].expr, &mut reads);
    for read in reads {
        if let Some(&producer) = producers.get(&read) {
            visit_assignment(producer, spec, producers, marks, order)?;
        } else if !spec.inputs.contains(&read) {
            return Err(err(format!(
                "assignment {} reads unknown tensor '{read}'",
                spec.assignments[index].primary_result()
            )));
        }
    }
    marks[index] = 2;
    order.push(index);
    Ok(())
}

fn collect_reads(expr: &ExprSpec, out: &mut BTreeSet<String>) {
    match expr {
        ExprSpec::Read { tensor, .. } => {
            out.insert(tensor.clone());
        }
        ExprSpec::Call { args, .. } | ExprSpec::MatrixMac { args, .. } => {
            for arg in args {
                collect_reads(arg, out);
            }
        }
        ExprSpec::Reduce { value, .. } => collect_reads(value, out),
    }
}

struct AssignmentValidation<'a> {
    spec: &'a ProgramSpec,
    bindings: &'a RegionBindings,
    producers: &'a BTreeMap<String, usize>,
    values: BTreeMap<String, TensorSpec>,
    extents: BTreeMap<String, usize>,
}

impl<'a> AssignmentValidation<'a> {
    fn new(
        spec: &'a ProgramSpec,
        bindings: &'a RegionBindings,
        producers: &'a BTreeMap<String, usize>,
    ) -> Self {
        let values = spec
            .inputs
            .iter()
            .zip(&bindings.inputs)
            .map(|(name, view)| {
                (
                    name.clone(),
                    TensorSpec {
                        shape: view.shape().dims().to_vec(),
                        dtype: *view.dtype(),
                    },
                )
            })
            .collect();
        Self {
            spec,
            bindings,
            producers,
            values,
            extents: spec.extents.clone(),
        }
    }

    fn validate(&mut self, assignment: &AssignmentSpec) -> Result<(), SimError> {
        let reduction_axes = self.validate_expression(assignment)?;
        let shape = self.output_shape(assignment, &reduction_axes)?;
        self.register_outputs(assignment, shape)
    }

    fn validate_expression(
        &mut self,
        assignment: &AssignmentSpec,
    ) -> Result<BTreeSet<String>, SimError> {
        let expr = &assignment.expr;
        let assignment_error = |error: SimError| {
            err(format!(
                "assignment {}: {}",
                assignment.primary_result(),
                error.0
            ))
        };
        validate_unique_input_mapping(expr, &mut BTreeMap::new()).map_err(assignment_error)?;
        validate_local_reads(expr, self.spec, self.producers).map_err(assignment_error)?;
        let mut read_axes = BTreeSet::new();
        let mut declared_reductions = BTreeSet::new();
        collect_axes(expr, &mut read_axes, &mut declared_reductions, false)
            .map_err(assignment_error)?;
        validate_executable_expr(expr).map_err(assignment_error)?;
        super::scaled::validate_scaled_matrix(expr, self.spec, &self.values)
            .map_err(assignment_error)?;
        let bound_axes = assignment
            .axes
            .iter()
            .cloned()
            .chain(declared_reductions.iter().cloned())
            .collect::<BTreeSet<_>>();
        if let Some(unbound) = read_axes.difference(&bound_axes).next() {
            return Err(err(format!(
                "assignment {} has unbound read axis {unbound}",
                assignment.primary_result()
            )));
        }
        if let Some(unused) = declared_reductions.difference(&read_axes).next() {
            return Err(err(format!(
                "assignment {} reduction axis {unused} is not read",
                assignment.primary_result()
            )));
        }
        let mut reduction_axes = BTreeSet::new();
        validate_expr(
            expr,
            &self.values,
            &mut self.extents,
            &mut reduction_axes,
            false,
        )
        .map_err(assignment_error)?;
        validate_required_ranges(expr, &self.values, &self.extents).map_err(assignment_error)?;
        validate_extremum_window(expr, &assignment.axes, &self.extents, &self.values)
            .map_err(assignment_error)?;
        Ok(reduction_axes)
    }

    fn output_shape(
        &self,
        assignment: &AssignmentSpec,
        reduction_axes: &BTreeSet<String>,
    ) -> Result<Vec<usize>, SimError> {
        let mut seen = BTreeSet::new();
        let mut shape = Vec::new();
        for axis in &assignment.axes {
            if !seen.insert(axis) || reduction_axes.contains(axis) {
                return Err(err(format!(
                    "assignment {} has duplicate or reduced output axis {axis}",
                    assignment.primary_result()
                )));
            }
            shape.push(*self.extents.get(axis).ok_or_else(|| {
                err(format!(
                    "assignment {} has unconstrained axis {axis}",
                    assignment.primary_result()
                ))
            })?);
        }
        for axis in reduction_axes {
            if !self.extents.contains_key(axis) {
                return Err(err(format!(
                    "assignment {} has unconstrained reduction axis {axis}",
                    assignment.primary_result()
                )));
            }
        }
        shape.iter().try_fold(1usize, |acc, size| {
            acc.checked_mul(*size)
                .ok_or_else(|| err("output shape product overflows"))
        })?;
        Ok(shape)
    }

    fn register_outputs(
        &mut self,
        assignment: &AssignmentSpec,
        shape: Vec<usize>,
    ) -> Result<(), SimError> {
        if let Some(value_name) = &assignment.output {
            let dtype = if let Some(temporary) = self.spec.temporaries.get(value_name) {
                temporary.dtype
            } else {
                let output_index = self
                    .spec
                    .outputs
                    .iter()
                    .position(|name| name == value_name)
                    .unwrap();
                let view = &self.bindings.outputs[output_index];
                if view.shape().dims() != shape {
                    return Err(err(format!(
                        "assignment {} shape {shape:?} differs from output {:?}",
                        value_name,
                        view.shape().dims()
                    )));
                }
                *view.dtype()
            };
            self.values.insert(
                value_name.clone(),
                TensorSpec {
                    shape: shape.clone(),
                    dtype,
                },
            );
        }
        if let Some(index_output) = &assignment.indices_output {
            let dtype = if let Some(temporary) = self.spec.temporaries.get(index_output) {
                temporary.dtype
            } else {
                let output_index = self
                    .spec
                    .outputs
                    .iter()
                    .position(|name| name == index_output)
                    .unwrap();
                let view = &self.bindings.outputs[output_index];
                if view.shape().dims() != shape {
                    return Err(err(format!(
                        "indices_output {index_output} must have shape {shape:?}"
                    )));
                }
                *view.dtype()
            };
            self.values
                .insert(index_output.clone(), TensorSpec { shape, dtype });
        }
        Ok(())
    }
}

fn validate_boundary_overlap(
    spec: &ProgramSpec,
    bindings: &RegionBindings,
) -> Result<(), SimError> {
    for (output_index, output) in bindings.outputs.iter().enumerate() {
        for (input_index, input) in bindings.inputs.iter().enumerate() {
            if output.first_overlapping_byte_ranges(input).is_some() {
                return Err(err(format!(
                    "output {} overlaps input {}",
                    spec.outputs[output_index], spec.inputs[input_index]
                )));
            }
        }
        for previous in 0..output_index {
            if output
                .first_overlapping_byte_ranges(&bindings.outputs[previous])
                .is_some()
            {
                return Err(err(format!(
                    "output {} overlaps output {}",
                    spec.outputs[output_index], spec.outputs[previous]
                )));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct RegionBindings {
    pub inputs: Vec<TensorView>,
    pub outputs: Vec<TensorView>,
}
fn validate_unique_input_mapping(
    expr: &ExprSpec,
    mappings: &mut BTreeMap<String, (Vec<IndexSpec>, BoundsMode)>,
) -> Result<(), SimError> {
    match expr {
        ExprSpec::Read {
            tensor,
            indices,
            bounds,
        } => match mappings.get(tensor) {
            Some((previous, mode)) if previous != indices || mode != bounds => Err(err(format!(
                "tensor {tensor} has multiple index mappings in one program"
            ))),
            _ => {
                mappings.insert(tensor.clone(), (indices.clone(), *bounds));
                Ok(())
            }
        },
        ExprSpec::Call { args, .. } | ExprSpec::MatrixMac { args, .. } => {
            for arg in args {
                validate_unique_input_mapping(arg, mappings)?;
            }
            Ok(())
        }
        ExprSpec::Reduce { value, .. } => validate_unique_input_mapping(value, mappings),
    }
}

fn validate_local_reads(
    expr: &ExprSpec,
    spec: &ProgramSpec,
    producers: &BTreeMap<String, usize>,
) -> Result<(), SimError> {
    match expr {
        ExprSpec::Read {
            tensor,
            indices,
            bounds,
        } if producers.contains_key(tensor) => {
            let producer = &spec.assignments[producers[tensor]];
            if *bounds != BoundsMode::RequireInRange
                || indices.len() != producer.axes.len()
                || indices.iter().zip(&producer.axes).any(|(index, axis)|
                    !matches!(index, IndexSpec::Axis(read_axis) if read_axis == axis))
            {
                return Err(err(format!(
                    "internal tensor {tensor} has an unsupported cross-tile dependency"
                )));
            }
        }
        ExprSpec::Read { .. } => {}
        ExprSpec::Call { args, .. } | ExprSpec::MatrixMac { args, .. } => {
            for arg in args {
                validate_local_reads(arg, spec, producers)?;
            }
        }
        ExprSpec::Reduce { value, .. } => {
            validate_local_reads(value, spec, producers)?;
        }
    }
    Ok(())
}

fn collect_axes(
    expr: &ExprSpec,
    read_axes: &mut BTreeSet<String>,
    reduction_axes: &mut BTreeSet<String>,
    inside_reduction: bool,
) -> Result<(), SimError> {
    match expr {
        ExprSpec::Read { indices, .. } => {
            for index in indices {
                match index {
                    IndexSpec::Axis(axis) => {
                        read_axes.insert(axis.clone());
                    }
                    IndexSpec::Affine { affine } => {
                        read_axes.extend(affine.terms.keys().cloned());
                    }
                    IndexSpec::Zero(_) => {}
                }
            }
        }
        ExprSpec::Call { args, .. } => {
            for arg in args {
                collect_axes(arg, read_axes, reduction_axes, inside_reduction)?;
            }
        }
        ExprSpec::MatrixMac { args, over, .. } => {
            if !over.is_empty() {
                if inside_reduction {
                    return Err(err("nested reduction is unsupported"));
                }
                reduction_axes.extend(over.iter().cloned());
            }
            for arg in args {
                collect_axes(
                    arg,
                    read_axes,
                    reduction_axes,
                    inside_reduction || !over.is_empty(),
                )?;
            }
        }
        ExprSpec::Reduce { over, value, .. } => {
            if inside_reduction {
                return Err(err("nested reduction is unsupported"));
            }
            reduction_axes.extend(over.iter().cloned());
            collect_axes(value, read_axes, reduction_axes, true)?;
        }
    }
    Ok(())
}

fn validate_executable_expr(expr: &ExprSpec) -> Result<(), SimError> {
    let direct_read = |value: &ExprSpec| matches!(value, ExprSpec::Read { .. });
    match expr {
        ExprSpec::Read { .. } => Ok(()),
        ExprSpec::Call { op, .. } if !is_lowercase_operation(op) => {
            Err(err(format!("operation '{op}' must be lowercase")))
        }
        ExprSpec::Call { op, .. } if matches!(op.as_str(), "read" | "reduce" | "matrix_mac") => {
            Err(err(format!(
                "operation '{op}' uses a reserved expression name"
            )))
        }
        ExprSpec::MatrixMac { args, over, .. } => {
            if over.len() != 1 || args.len() != 2 || !args.iter().all(direct_read) {
                return Err(err(
                    "matrix_mac requires two direct reads, one reduction axis, and no attributes",
                ));
            }
            for arg in args {
                let ExprSpec::Read {
                    indices, bounds, ..
                } = arg
                else {
                    unreachable!()
                };
                if *bounds != BoundsMode::RequireInRange
                    || indices
                        .iter()
                        .any(|index| matches!(index, IndexSpec::Affine { .. }))
                    || !indices
                        .iter()
                        .any(|index| matches!(index, IndexSpec::Axis(axis) if axis == &over[0]))
                {
                    return Err(err(
                        "matrix_mac operands must directly index the reduction axis",
                    ));
                }
            }
            Ok(())
        }
        ExprSpec::Call { args, .. } if !args.is_empty() => {
            args.iter().try_for_each(validate_executable_expr)
        }
        ExprSpec::Reduce {
            op: ReductionOp::Sum,
            value,
            ..
        } if direct_read(value) || matches!(&**value, ExprSpec::Call { .. }) => {
            validate_executable_expr(value)
        }
        ExprSpec::Reduce { value, .. } if direct_read(value) => Ok(()),
        ExprSpec::Reduce { value, .. } if matches!(&**value, ExprSpec::Call { .. }) => {
            common_excluded_read(value)?;
            validate_executable_expr(value)
        }
        _ => Err(err(
            "unsupported nested arithmetic; use an explicit temporary assignment",
        )),
    }
}

fn validate_expr(
    expr: &ExprSpec,
    values: &BTreeMap<String, TensorSpec>,
    extents: &mut BTreeMap<String, usize>,
    reduction_axes: &mut BTreeSet<String>,
    in_reduction: bool,
) -> Result<(), SimError> {
    match expr {
        ExprSpec::Read {
            tensor,
            indices,
            bounds,
        } => {
            let value = values
                .get(tensor)
                .ok_or_else(|| err(format!("unknown tensor '{tensor}'")))?;
            if indices.len() != value.shape.len() {
                return Err(err(format!(
                    "tensor {tensor} index rank {} differs from shape rank {}",
                    indices.len(),
                    value.shape.len()
                )));
            }
            if *bounds == BoundsMode::Exclude && !in_reduction {
                return Err(err(format!(
                    "tensor {tensor} uses exclude bounds outside a reduction"
                )));
            }
            let mut used_axes = BTreeSet::new();
            for (index, size) in indices.iter().zip(&value.shape) {
                match index {
                    IndexSpec::Axis(axis) => {
                        if !used_axes.insert(axis.clone()) {
                            return Err(err(format!(
                                "tensor {tensor} repeats axis {axis} in one read"
                            )));
                        }
                        axis_extent(extents, axis, *size, tensor)?;
                    }
                    IndexSpec::Zero(0) => {
                        if *size != 1 {
                            return Err(err(format!(
                                "tensor {tensor} literal-zero index requires a singleton dimension"
                            )));
                        }
                    }
                    IndexSpec::Zero(_) => {
                        return Err(err(format!(
                            "tensor {tensor} supports only literal-zero indices"
                        )));
                    }
                    IndexSpec::Affine { affine } => {
                        for (axis, coefficient) in &affine.terms {
                            if !used_axes.insert(axis.clone()) {
                                return Err(err(format!(
                                    "tensor {tensor} repeats axis {axis} in one read"
                                )));
                            }
                            if *coefficient == 0 {
                                return Err(err(format!(
                                    "tensor {tensor} affine coefficient for {axis} is zero"
                                )));
                            }
                            if !extents.contains_key(axis) {
                                return Err(err(format!(
                                    "tensor {tensor} affine axis {axis} has no declared extent"
                                )));
                            }
                        }
                    }
                }
            }
        }
        ExprSpec::Call { args, .. } => {
            for arg in args {
                validate_expr(arg, values, extents, reduction_axes, in_reduction)?;
            }
        }
        ExprSpec::MatrixMac { args, over, .. } => {
            for axis in over {
                if !reduction_axes.insert(axis.clone()) {
                    return Err(err(format!("duplicate reduction axis {axis}")));
                }
            }
            for arg in args {
                validate_expr(arg, values, extents, reduction_axes, in_reduction)?;
            }
        }
        ExprSpec::Reduce { over, value, .. } => {
            if over.is_empty() {
                return Err(err("reduction must name at least one axis"));
            }
            for axis in over {
                if !reduction_axes.insert(axis.clone()) {
                    return Err(err(format!("duplicate reduction axis {axis}")));
                }
            }
            validate_expr(value, values, extents, reduction_axes, true)?;
        }
    }
    Ok(())
}

fn validate_required_ranges(
    expr: &ExprSpec,
    values: &BTreeMap<String, TensorSpec>,
    extents: &BTreeMap<String, usize>,
) -> Result<(), SimError> {
    match expr {
        ExprSpec::Read {
            tensor,
            indices,
            bounds: BoundsMode::RequireInRange,
        } => {
            let shape = &values
                .get(tensor)
                .ok_or_else(|| err(format!("unknown tensor {tensor}")))?
                .shape;
            for (index, dimension) in indices.iter().zip(shape) {
                let (first, last) = match index {
                    IndexSpec::Zero(0) => (0, 0),
                    IndexSpec::Zero(_) => unreachable!(),
                    IndexSpec::Axis(axis) => (
                        0,
                        i128::try_from(extents[axis] - 1)
                            .map_err(|_| err("index extent overflows"))?,
                    ),
                    IndexSpec::Affine { affine } => {
                        let last = affine.terms.iter().try_fold(
                            i128::from(affine.offset),
                            |sum, (axis, coefficient)| {
                                let extent = i128::try_from(extents[axis] - 1)
                                    .map_err(|_| err("index extent overflows"))?;
                                let coefficient = i128::try_from(*coefficient)
                                    .map_err(|_| err("index coefficient overflows"))?;
                                sum.checked_add(
                                    extent
                                        .checked_mul(coefficient)
                                        .ok_or_else(|| err("index range overflows"))?,
                                )
                                .ok_or_else(|| err("index range overflows"))
                            },
                        )?;
                        (i128::from(affine.offset), last)
                    }
                };
                if *dimension == 1 {
                    continue;
                }
                if first < 0
                    || last
                        >= i128::try_from(*dimension)
                            .map_err(|_| err("index dimension overflows"))?
                {
                    return Err(err(format!(
                        "tensor {tensor} read index exceeds its dimension"
                    )));
                }
            }
        }
        ExprSpec::Read { .. } => {}
        ExprSpec::Call { args, .. } | ExprSpec::MatrixMac { args, .. } => {
            for arg in args {
                validate_required_ranges(arg, values, extents)?;
            }
        }
        ExprSpec::Reduce { value, .. } => {
            validate_required_ranges(value, values, extents)?;
        }
    }
    Ok(())
}

fn validate_extremum_window(
    expr: &ExprSpec,
    output_axes: &[String],
    extents: &BTreeMap<String, usize>,
    values: &BTreeMap<String, TensorSpec>,
) -> Result<(), SimError> {
    let ExprSpec::Reduce {
        op: ReductionOp::Min | ReductionOp::Max,
        value,
        ..
    } = expr
    else {
        return Ok(());
    };
    let Some(ExprSpec::Read {
        tensor, indices, ..
    }) = common_excluded_read(value)?
    else {
        return Ok(());
    };
    let shape = &values
        .get(tensor)
        .ok_or_else(|| err(format!("unknown max source {tensor}")))?
        .shape;
    for upper_output_corner in [false, true] {
        for (index, dimension) in indices.iter().zip(shape) {
            let (min, max) = match index {
                IndexSpec::Zero(_) => (0i128, 0i128),
                IndexSpec::Axis(axis) => {
                    let position = if output_axes.contains(axis) && upper_output_corner {
                        extents[axis] - 1
                    } else {
                        0
                    };
                    let position =
                        i128::try_from(position).map_err(|_| err("window index overflows"))?;
                    (position, position)
                }
                IndexSpec::Affine { affine } => {
                    let mut min = i128::from(affine.offset);
                    let mut max = min;
                    for (axis, coefficient) in &affine.terms {
                        let end = i128::try_from(extents[axis] - 1)
                            .map_err(|_| err("window extent overflows"))?
                            .checked_mul(
                                i128::try_from(*coefficient)
                                    .map_err(|_| err("window coefficient overflows"))?,
                            )
                            .ok_or_else(|| err("window affine index overflows"))?;
                        if output_axes.contains(axis) {
                            if upper_output_corner {
                                min = min
                                    .checked_add(end)
                                    .ok_or_else(|| err("window affine index overflows"))?;
                                max = max
                                    .checked_add(end)
                                    .ok_or_else(|| err("window affine index overflows"))?;
                            }
                        } else {
                            max = max
                                .checked_add(end)
                                .ok_or_else(|| err("window affine index overflows"))?;
                        }
                    }
                    (min, max)
                }
            };
            let dimension =
                i128::try_from(*dimension).map_err(|_| err("window dimension overflows"))?;
            if dimension == 1 {
                continue;
            }
            if min >= dimension || max < 0 {
                return Err(err(format!(
                    "min/max window on {tensor} has an empty valid range"
                )));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TensorSpec {
    pub shape: Vec<usize>,
    pub dtype: DataType,
}

fn axis_extent(
    extents: &mut BTreeMap<String, usize>,
    axis: &str,
    size: usize,
    tensor: &str,
) -> Result<(), SimError> {
    if size == 1 {
        extents.entry(axis.to_owned()).or_insert(1);
        return Ok(());
    }
    match extents.get(axis) {
        Some(existing) if *existing != size && *existing != 1 => Err(err(format!(
            "tensor {tensor} axis {axis} has extent {size}, expected {existing}"
        ))),
        _ => {
            extents.insert(axis.to_owned(), size);
            Ok(())
        }
    }
}

fn register(names: &mut BTreeSet<String>, name: &str) -> Result<(), SimError> {
    if name.is_empty() || !names.insert(name.to_owned()) {
        return Err(err(format!(
            "duplicate or empty program tensor name '{name}'"
        )));
    }
    Ok(())
}

fn common_excluded_read(expr: &ExprSpec) -> Result<Option<&ExprSpec>, SimError> {
    match expr {
        ExprSpec::Read {
            bounds: BoundsMode::Exclude,
            ..
        } => Ok(Some(expr)),
        ExprSpec::Call { args, .. } => {
            let mut common = None;
            for arg in args {
                if let Some(read) = common_excluded_read(arg)? {
                    if common.is_some_and(|prior| prior != read) {
                        return Err(err(
                            "nested extremum operands require a common excluded mapping",
                        ));
                    }
                    common = Some(read);
                }
            }
            Ok(common)
        }
        _ => Ok(None),
    }
}

fn err(message: impl Into<String>) -> SimError {
    SimError(message.into())
}
