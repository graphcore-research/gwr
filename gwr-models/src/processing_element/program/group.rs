// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Invariant facts for one lazy tile group, using original assignment
//! identities.

use std::collections::{BTreeMap, BTreeSet};

use super::analysis::access::GroupInputs;
use super::validate::ProgramFacts;

#[derive(Debug)]
pub(in crate::processing_element) struct GroupMetadata {
    /// Topological order used by analysis and local allocation.
    pub assignments: Vec<usize>,
    /// Stable runtime order: source order for flat tiles, group order
    /// otherwise.
    pub issue_order: Vec<usize>,
    pub dependencies: BTreeMap<usize, BTreeSet<usize>>,
    pub inputs: GroupInputs,
    pub input_dependencies: BTreeMap<usize, BTreeSet<String>>,
    pub outputs: Vec<GroupOutput>,
    pub layers: BTreeMap<usize, usize>,
    pub last_consumer_layers: BTreeMap<String, usize>,
}

#[derive(Debug)]
pub(in crate::processing_element) struct GroupOutput {
    pub binding: usize,
    pub producer: usize,
}

impl GroupMetadata {
    pub fn new(region: &ProgramFacts, assignments: Vec<usize>) -> Self {
        let dependencies = assignments
            .iter()
            .map(|&index| {
                (
                    index,
                    region.dependencies[index]
                        .iter()
                        .copied()
                        .filter(|i| assignments.contains(i))
                        .collect(),
                )
            })
            .collect();
        let inputs = GroupInputs::new(region, &assignments);
        let initial_inputs = inputs
            .initial
            .iter()
            .map(|read| region.program.inputs[read.input].clone())
            .collect::<BTreeSet<_>>();
        let input_dependencies = assignments
            .iter()
            .map(|&index| {
                (
                    index,
                    region.program.assignments[index]
                        .expr
                        .collect_read_tensor_names()
                        .intersection(&initial_inputs)
                        .cloned()
                        .collect(),
                )
            })
            .collect();
        let outputs = region
            .program
            .outputs
            .iter()
            .enumerate()
            .filter_map(|(binding, name)| {
                let producer = region.producers[name];
                assignments
                    .contains(&producer)
                    .then_some(GroupOutput { binding, producer })
            })
            .collect();
        let layers = calculate_assignment_layers(region, &assignments);
        let last_consumer_layers = region
            .consumers
            .iter()
            .filter_map(|(name, consumers)| {
                consumers
                    .iter()
                    .filter_map(|i| layers.get(i))
                    .max()
                    .map(|last| (name.clone(), *last))
            })
            .collect();
        Self {
            issue_order: assignments.clone(),
            assignments,
            dependencies,
            inputs,
            input_dependencies,
            outputs,
            layers,
            last_consumer_layers,
        }
    }
}

/// Dependencies without a producer in this group belong to an enclosing scope.
fn calculate_assignment_layers(
    region: &ProgramFacts,
    assignments: &[usize],
) -> BTreeMap<usize, usize> {
    let mut layers = BTreeMap::new();
    for &index in assignments {
        let layer = region.dependencies[index]
            .iter()
            .filter_map(|dependency| layers.get(dependency).copied())
            .max()
            .map_or(0, |layer: usize| layer + 1);
        layers.insert(index, layer);
    }
    layers
}
