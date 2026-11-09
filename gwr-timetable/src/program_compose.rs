// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Expand reusable program calls into the flat tensor program executed by a PE.

use std::collections::{BTreeMap, BTreeSet};

use gwr_engine::sim_error;
use gwr_engine::types::SimError;
use gwr_models::processing_element::program::syntax::{
    AssignmentSpec, ExprSpec, IndexSpec, ProgramSpec,
};

use crate::timetable_file::{ProgramCall, ProgramDefinition, ProgramSection};

pub(crate) struct ProgramCatalog<'a> {
    definitions: &'a BTreeMap<String, ProgramDefinition>,
    expanded: BTreeMap<String, ProgramSpec>,
}

impl<'a> ProgramCatalog<'a> {
    pub(crate) fn new(definitions: &'a BTreeMap<String, ProgramDefinition>) -> Self {
        Self {
            definitions,
            expanded: BTreeMap::new(),
        }
    }

    pub(crate) fn resolve(
        &mut self,
        section: ProgramSection,
        node_id: &str,
    ) -> Result<ProgramSpec, SimError> {
        let program = match section {
            ProgramSection::Named(name) => self.expand_named(&name, &mut Vec::new()),
            ProgramSection::Inline(definition) => {
                self.expand_definition(&definition, &mut Vec::new())
            }
        };
        program.map_err(|error| SimError(format!("Compute node '{node_id}': {}", error.0)))
    }

    fn expand_named(
        &mut self,
        name: &str,
        stack: &mut Vec<String>,
    ) -> Result<ProgramSpec, SimError> {
        if let Some(program) = self.expanded.get(name) {
            return Ok(program.clone());
        }
        if stack.iter().any(|part| part == name) {
            return sim_error!("program call cycle: {} -> {name}", stack.join(" -> "));
        }
        let definition = self
            .definitions
            .get(name)
            .ok_or_else(|| SimError(format!("unknown program '{name}'")))?
            .clone();
        stack.push(name.to_owned());
        let result = self.expand_definition(&definition, stack);
        stack.pop();
        let program = result.map_err(|error| SimError(format!("program '{name}': {}", error.0)))?;
        self.expanded.insert(name.to_owned(), program.clone());
        Ok(program)
    }

    fn expand_definition(
        &mut self,
        definition: &ProgramDefinition,
        stack: &mut Vec<String>,
    ) -> Result<ProgramSpec, SimError> {
        let indexed = definition
            .indexed_metadata_inputs
            .iter()
            .collect::<BTreeSet<_>>();
        if indexed.len() != definition.indexed_metadata_inputs.len()
            || indexed
                .iter()
                .any(|name| !definition.metadata_inputs.contains(name))
        {
            return sim_error!(
                "indexed_metadata_inputs must be a distinct subset of metadata_inputs"
            );
        }

        if definition.local_round_policy.is_some() && !definition.calls.is_empty() {
            return sim_error!("local round policy does not support composition");
        }
        let mut declarations = BTreeSet::new();
        for name in definition
            .inputs
            .iter()
            .chain(&definition.outputs)
            .chain(definition.temporaries.keys())
        {
            if name.is_empty() || !declarations.insert(name) {
                return sim_error!("duplicate or empty program value name '{name}'");
            }
        }
        let mut flat = ProgramSpec {
            inputs: definition.inputs.clone(),
            metadata_inputs: definition.metadata_inputs.clone(),
            indexed_metadata_inputs: definition.indexed_metadata_inputs.clone(),
            local_round_policy: definition.local_round_policy,
            outputs: definition.outputs.clone(),
            temporaries: definition.temporaries.clone(),
            assignments: definition.assignments.clone(),
            extents: definition.extents.clone(),
        };
        let mut used_names = declarations.into_iter().cloned().collect::<BTreeSet<_>>();
        for (index, call) in definition.calls.iter().enumerate() {
            let callee = self.expand_named(&call.program, stack)?;
            append_call(&mut flat, &mut used_names, call, index, &callee)
                .map_err(|error| SimError(format!("call to '{}': {}", call.program, error.0)))?;
        }
        Ok(flat)
    }
}

fn append_call(
    flat: &mut ProgramSpec,
    used_names: &mut BTreeSet<String>,
    call: &ProgramCall,
    index: usize,
    callee: &ProgramSpec,
) -> Result<(), SimError> {
    if flat.local_round_policy.is_some() || callee.local_round_policy.is_some() {
        return sim_error!("local round policy does not support composed/remapped calls");
    }
    check_ports("input", &callee.inputs, &call.inputs)?;
    check_ports("output", &callee.outputs, &call.outputs)?;
    for target in call.inputs.values() {
        if !used_names.contains(target) {
            return sim_error!("input binds unknown value '{target}'");
        }
    }
    for target in call.outputs.values() {
        if !flat.outputs.contains(target) && !flat.temporaries.contains_key(target) {
            return sim_error!("output binds undeclared value '{target}'");
        }
    }

    let axes = program_axes(callee);
    for source in call.axes.keys() {
        if !axes.contains(source) {
            return sim_error!("unknown axis '{source}'");
        }
    }
    let renamed_axes = axes
        .iter()
        .map(|axis| call.axes.get(axis).unwrap_or(axis))
        .collect::<BTreeSet<_>>();
    if renamed_axes.len() != axes.len() || renamed_axes.iter().any(|axis| axis.is_empty()) {
        return sim_error!("axis bindings must be distinct and nonempty");
    }

    let mut names = call.inputs.clone();
    names.extend(call.outputs.clone());
    for (name, temporary) in &callee.temporaries {
        let base = format!("__call{index}_{name}");
        let mut generated = base.clone();
        let mut suffix = 2;
        while used_names.contains(&generated) {
            generated = format!("{base}_{suffix}");
            suffix += 1;
        }
        used_names.insert(generated.clone());
        names.insert(name.clone(), generated.clone());
        flat.temporaries.insert(generated, temporary.clone());
    }
    for metadata in &callee.metadata_inputs {
        let mapped = mapped_name(&names, metadata)?;
        if !flat.inputs.contains(&mapped) {
            return sim_error!(
                "metadata input '{metadata}' binds local value '{mapped}'; metadata must bind an external input"
            );
        }
        let indexed = callee.indexed_metadata_inputs.contains(metadata);
        if flat.metadata_inputs.contains(&mapped) {
            if flat.indexed_metadata_inputs.contains(&mapped) != indexed {
                return sim_error!(
                    "metadata input '{mapped}' mixes full-view and indexed declarations"
                );
            }
        } else {
            flat.metadata_inputs.push(mapped.clone());
            if indexed {
                flat.indexed_metadata_inputs.push(mapped);
            }
        }
    }
    for (axis, extent) in &callee.extents {
        let renamed = rename_axis(axis, &call.axes);
        if let Some(previous) = flat.extents.insert(renamed.clone(), *extent)
            && previous != *extent
        {
            return sim_error!("axis '{renamed}' has conflicting extents {previous} and {extent}");
        }
    }
    for assignment in &callee.assignments {
        let mut assignment: AssignmentSpec = assignment.clone();
        assignment.output = assignment
            .output
            .map(|name| mapped_name(&names, &name))
            .transpose()?;
        assignment.indices_output = assignment
            .indices_output
            .map(|name| mapped_name(&names, &name))
            .transpose()?;
        assignment.axes = assignment
            .axes
            .iter()
            .map(|axis| rename_axis(axis, &call.axes))
            .collect();
        rename_expr(&mut assignment.expr, &names, &call.axes)?;
        flat.assignments.push(assignment);
    }
    Ok(())
}

fn check_ports(
    direction: &str,
    expected: &[String],
    supplied: &BTreeMap<String, String>,
) -> Result<(), SimError> {
    let expected = expected.iter().cloned().collect::<BTreeSet<_>>();
    let actual = supplied.keys().cloned().collect::<BTreeSet<_>>();
    if expected != actual {
        return sim_error!(
            "{direction} bindings differ: missing {:?}, unknown {:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        );
    }
    Ok(())
}

fn program_axes(program: &ProgramSpec) -> BTreeSet<String> {
    let mut axes = program.extents.keys().cloned().collect::<BTreeSet<_>>();
    for assignment in &program.assignments {
        axes.extend(assignment.axes.iter().cloned());
        expr_axes(&assignment.expr, &mut axes);
    }
    axes
}

fn expr_axes(expr: &ExprSpec, axes: &mut BTreeSet<String>) {
    match expr {
        ExprSpec::Read { indices, .. } => {
            for index in indices {
                match index {
                    IndexSpec::Axis(axis) => {
                        axes.insert(axis.clone());
                    }
                    IndexSpec::Affine { affine } => {
                        axes.extend(affine.terms.keys().cloned());
                    }
                    IndexSpec::Zero(_) => {}
                }
            }
        }
        ExprSpec::Call { args, .. } => {
            for arg in args {
                expr_axes(arg, axes);
            }
        }
        ExprSpec::MatrixMac { args, over, .. } => {
            axes.extend(over.iter().cloned());
            for arg in args {
                expr_axes(arg, axes);
            }
        }
        ExprSpec::Reduce { over, value, .. } => {
            axes.extend(over.iter().cloned());
            expr_axes(value, axes);
        }
    }
}

fn rename_expr(
    expr: &mut ExprSpec,
    names: &BTreeMap<String, String>,
    axes: &BTreeMap<String, String>,
) -> Result<(), SimError> {
    match expr {
        ExprSpec::Read {
            tensor, indices, ..
        } => {
            *tensor = mapped_name(names, tensor)?;
            for index in indices {
                match index {
                    IndexSpec::Axis(axis) => *axis = rename_axis(axis, axes),
                    IndexSpec::Affine { affine } => {
                        affine.terms = affine
                            .terms
                            .iter()
                            .map(|(axis, value)| (rename_axis(axis, axes), *value))
                            .collect();
                    }
                    IndexSpec::Zero(_) => {}
                }
            }
        }
        ExprSpec::Call { args, .. } => {
            for arg in args {
                rename_expr(arg, names, axes)?;
            }
        }
        ExprSpec::MatrixMac { args, over, .. } => {
            *over = over.iter().map(|axis| rename_axis(axis, axes)).collect();
            for arg in args {
                rename_expr(arg, names, axes)?;
            }
        }
        ExprSpec::Reduce { over, value, .. } => {
            *over = over.iter().map(|axis| rename_axis(axis, axes)).collect();
            rename_expr(value, names, axes)?;
        }
    }
    Ok(())
}

fn rename_axis(axis: &str, axes: &BTreeMap<String, String>) -> String {
    axes.get(axis).cloned().unwrap_or_else(|| axis.to_owned())
}

fn mapped_name(names: &BTreeMap<String, String>, source: &str) -> Result<String, SimError> {
    names
        .get(source)
        .cloned()
        .ok_or_else(|| SimError(format!("called program uses undeclared value '{source}'")))
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    #[test]
    fn composed_metadata_follows_port_renames_and_deduplicates_calls() {
        let definitions: BTreeMap<String, ProgramDefinition> = yaml_serde::from_str(
            r"
leaf:
  inputs: [X, scale]
  metadata_inputs: [scale]
  outputs: [Y]
  assignments: [{ output: Y, axes: [i], expr: { read: { tensor: X, indices: [i] } } }]
wrapper:
  inputs: [A, S]
  metadata_inputs: [S]
  outputs: [Y0, Y1]
  calls:
    - { program: leaf, inputs: { X: A, scale: S }, outputs: { Y: Y0 } }
    - { program: leaf, inputs: { X: A, scale: S }, outputs: { Y: Y1 } }
",
        )
        .unwrap();
        let program = ProgramCatalog::new(&definitions)
            .resolve(ProgramSection::Named("wrapper".into()), "test")
            .unwrap();
        assert_eq!(program.metadata_inputs, vec!["S"]);
        assert_eq!(program.assignments.len(), 2);
        let converted = ProgramDefinition::from(program);
        assert_eq!(converted.metadata_inputs, vec!["S"]);
    }

    #[test]
    fn metadata_binding_cannot_be_a_namespaced_temporary() {
        let definitions: BTreeMap<String, ProgramDefinition> = yaml_serde::from_str(
            r"
leaf:
  inputs: [X, scale]
  metadata_inputs: [scale]
  outputs: [Y]
  assignments: [{ output: Y, axes: [i], expr: { read: { tensor: X, indices: [i] } } }]
wrapper:
  inputs: [A]
  outputs: [Y]
  temporaries: { T: { dtype: fp32 } }
  calls:
    - { program: leaf, inputs: { X: A, scale: T }, outputs: { Y: Y } }
",
        )
        .unwrap();
        let error = ProgramCatalog::new(&definitions)
            .resolve(ProgramSection::Named("wrapper".into()), "test")
            .unwrap_err();
        assert!(error.0.contains("metadata must bind an external input"));
    }

    fn indexed_definitions() -> BTreeMap<String, ProgramDefinition> {
        yaml_serde::from_str(r"
leaf:
  inputs: [scale]
  metadata_inputs: [scale]
  indexed_metadata_inputs: [scale]
  outputs: [Y]
  assignments:
  - {output: Y, axes: [n], expr: {read: {tensor: scale, indices: [n, {affine: {terms: {}, offset: 1}}]}}}
wrapper:
  inputs: [S]
  metadata_inputs: [S]
  indexed_metadata_inputs: [S]
  outputs: [Y0, Y1]
  calls:
  - {program: leaf, inputs: {scale: S}, outputs: {Y: Y0}, axes: {n: row}}
  - {program: leaf, inputs: {scale: S}, outputs: {Y: Y1}, axes: {n: row}}
").unwrap()
    }

    #[test]
    fn indexed_composition_remaps_names_axes_and_preserves_all_read_leaves() {
        let definitions = indexed_definitions();
        let expanded = ProgramCatalog::new(&definitions)
            .resolve(ProgramSection::Named("wrapper".into()), "valid")
            .unwrap();
        assert_eq!(expanded.metadata_inputs, vec!["S"]);
        assert_eq!(expanded.indexed_metadata_inputs, vec!["S"]);
        assert_eq!(expanded.assignments.len(), 2);
        for assignment in &expanded.assignments {
            assert_eq!(assignment.axes, vec!["row"]);
            let ExprSpec::Read {
                tensor, indices, ..
            } = &assignment.expr
            else {
                unreachable!()
            };
            assert_eq!(tensor, "S");
            assert!(matches!(&indices[0], IndexSpec::Axis(axis) if axis == "row"));
            assert!(matches!(&indices[1], IndexSpec::Affine { affine }
                if affine.terms.is_empty() && affine.offset == 1));
        }
        let wire = ProgramDefinition::from(expanded.clone());
        assert_eq!(wire.indexed_metadata_inputs, vec!["S"]);
        let reparsed: ProgramDefinition =
            yaml_serde::from_str(&yaml_serde::to_string(&wire).unwrap()).unwrap();
        assert_eq!(reparsed.indexed_metadata_inputs, vec!["S"]);
        let bindings = gwr_models::processing_element::program::validate::RegionBindings {
            inputs: vec![
                gwr_models::processing_element::tensors::TensorView::new_full(
                    gwr_models::processing_element::tensors::Tensor::new(
                        &[4, 3],
                        &gwr_models::processing_element::tensors::dtype::DataType::Uint8,
                        100,
                    )
                    .unwrap(),
                ),
            ],
            outputs: [300, 400]
                .into_iter()
                .map(|address| {
                    gwr_models::processing_element::tensors::TensorView::new_full(
                        gwr_models::processing_element::tensors::Tensor::new(
                            &[4],
                            &gwr_models::processing_element::tensors::dtype::DataType::Uint8,
                            address,
                        )
                        .unwrap(),
                    )
                })
                .collect(),
        };
        gwr_models::processing_element::program::validate::validate_program(&expanded, &bindings)
            .unwrap();
    }

    #[test]
    fn full_view_indexed_conflicts_and_unknown_policy_reject_valid_first() {
        let original = indexed_definitions();
        ProgramCatalog::new(&original)
            .resolve(ProgramSection::Named("wrapper".into()), "valid")
            .unwrap();
        for name in ["wrapper", "leaf"] {
            let mut conflicting = original.clone();
            conflicting
                .get_mut(name)
                .unwrap()
                .indexed_metadata_inputs
                .clear();
            let error = ProgramCatalog::new(&conflicting)
                .resolve(ProgramSection::Named("wrapper".into()), "invalid")
                .unwrap_err();
            assert!(
                error.0.contains("mixes full-view and indexed declarations"),
                "{error:?}"
            );
        }
        let mut unknown = original.clone();
        unknown.get_mut("leaf").unwrap().indexed_metadata_inputs = vec!["undeclared".into()];
        let error = ProgramCatalog::new(&unknown)
            .resolve(ProgramSection::Named("wrapper".into()), "invalid")
            .unwrap_err();
        assert!(
            error.0.contains("distinct subset of metadata_inputs"),
            "{error:?}"
        );
    }
}
