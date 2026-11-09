// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashSet, VecDeque};
use std::fmt;
use std::ops::Range;

use gwr_engine::sim_error;
use gwr_engine::types::SimResult;
use gwr_models::processing_element::tensors::TensorView;

use super::{ComputeTensorDirection, TimetableEdge, TimetableGraph};

pub(super) fn validate(graph: &TimetableGraph) -> SimResult {
    let mut accesses = graph
        .edges()
        .iter()
        .filter_map(TimetableEdge::tensor_connection)
        .map(|connection| TensorAccess {
            node: connection.compute_node(),
            tensor: connection.tensor_node(),
            kind: match connection.direction() {
                ComputeTensorDirection::Input => AccessKind::Read,
                ComputeTensorDirection::Output => AccessKind::Write,
            },
            view: connection.view(),
        })
        .collect::<Vec<_>>();
    accesses.sort_by_key(|access| access.view.address_bounds().start);

    let mut dependencies = DependencyOrder::new(graph);
    let mut active_reads = BTreeSet::new();
    let mut active_writes = BTreeSet::new();
    let mut ending = BinaryHeap::new();
    for (current_index, current) in accesses.iter().copied().enumerate() {
        let current_bounds = current.view.address_bounds();
        while ending
            .peek()
            .is_some_and(|Reverse((end, _))| *end <= current_bounds.start)
        {
            let Reverse((_, index)) = ending.pop().expect("peeked access is present");
            active_reads.remove(&index);
            active_writes.remove(&index);
        }

        for candidate_index in conflict_candidates(current.kind, &active_reads, &active_writes) {
            let candidate = accesses[*candidate_index];
            if candidate.view.address_bounds().start >= current_bounds.end {
                continue;
            }
            let Some(ranges) = candidate.view.first_overlapping_byte_ranges(current.view) else {
                continue;
            };
            if candidate.node == current.node
                || !dependencies.are_ordered(current.node, candidate.node)
            {
                return AccessConflict {
                    first: candidate,
                    second: current,
                    ranges,
                }
                .into_error(graph);
            }
        }

        match current.kind {
            AccessKind::Read => active_reads.insert(current_index),
            AccessKind::Write => active_writes.insert(current_index),
        };
        ending.push(Reverse((current_bounds.end, current_index)));
    }
    Ok(())
}

fn conflict_candidates<'a>(
    kind: AccessKind,
    reads: &'a BTreeSet<usize>,
    writes: &'a BTreeSet<usize>,
) -> impl Iterator<Item = &'a usize> {
    let reads = (kind == AccessKind::Write).then_some(reads);
    writes.iter().chain(reads.into_iter().flatten())
}

struct AccessConflict<'a> {
    first: TensorAccess<'a>,
    second: TensorAccess<'a>,
    ranges: (Range<u128>, Range<u128>),
}

impl AccessConflict<'_> {
    fn into_error(self, graph: &TimetableGraph) -> SimResult {
        let first_node = graph.nodes()[self.first.node].id();
        let second_node = graph.nodes()[self.second.node].id();
        let first_tensor = graph.nodes()[self.first.tensor].id();
        let second_tensor = graph.nodes()[self.second.tensor].id();
        let (first_range, second_range) = self.ranges;

        match (self.first.kind, self.second.kind) {
            (AccessKind::Read, AccessKind::Write) if self.first.node == self.second.node => {
                sim_error!(
                    "Node '{first_node}' reads tensor '{first_tensor}' from memory range {:#x}..{:#x} and writes tensor '{second_tensor}' to overlapping range {:#x}..{:#x}",
                    first_range.start,
                    first_range.end,
                    second_range.start,
                    second_range.end,
                )
            }
            (AccessKind::Write, AccessKind::Read) if self.first.node == self.second.node => {
                sim_error!(
                    "Node '{second_node}' reads tensor '{second_tensor}' from memory range {:#x}..{:#x} and writes tensor '{first_tensor}' to overlapping range {:#x}..{:#x}",
                    second_range.start,
                    second_range.end,
                    first_range.start,
                    first_range.end,
                )
            }
            (AccessKind::Read, AccessKind::Write) => sim_error!(
                "Node '{first_node}' reads tensor '{first_tensor}' from memory range {:#x}..{:#x} while unordered node '{second_node}' writes tensor '{second_tensor}' to overlapping range {:#x}..{:#x}",
                first_range.start,
                first_range.end,
                second_range.start,
                second_range.end,
            ),
            (AccessKind::Write, AccessKind::Read) => sim_error!(
                "Node '{second_node}' reads tensor '{second_tensor}' from memory range {:#x}..{:#x} while unordered node '{first_node}' writes tensor '{first_tensor}' to overlapping range {:#x}..{:#x}",
                second_range.start,
                second_range.end,
                first_range.start,
                first_range.end,
            ),
            (AccessKind::Write, AccessKind::Write) if self.first.node == self.second.node => {
                sim_error!(
                    "Node '{first_node}' writes tensor '{first_tensor}' to memory range {:#x}..{:#x} and tensor '{second_tensor}' to overlapping range {:#x}..{:#x}",
                    first_range.start,
                    first_range.end,
                    second_range.start,
                    second_range.end,
                )
            }
            (AccessKind::Write, AccessKind::Write) if self.first.tensor == self.second.tensor => {
                sim_error!(
                    "Nodes '{first_node}' and '{second_node}' write tensor '{first_tensor}' to overlapping memory ranges {:#x}..{:#x} and {:#x}..{:#x}",
                    first_range.start,
                    first_range.end,
                    second_range.start,
                    second_range.end,
                )
            }
            (AccessKind::Write, AccessKind::Write) => sim_error!(
                "Nodes '{first_node}' and '{second_node}' write tensors '{first_tensor}' and '{second_tensor}' to overlapping memory ranges {:#x}..{:#x} and {:#x}..{:#x}",
                first_range.start,
                first_range.end,
                second_range.start,
                second_range.end,
            ),
            (AccessKind::Read, AccessKind::Read) => Ok(()),
        }
    }
}

#[derive(Clone, Copy)]
struct TensorAccess<'a> {
    node: usize,
    tensor: usize,
    kind: AccessKind,
    view: &'a TensorView,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum AccessKind {
    Read,
    Write,
}

impl fmt::Display for AccessKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read => formatter.write_str("reads"),
            Self::Write => formatter.write_str("writes"),
        }
    }
}

struct DependencyOrder<'a> {
    graph: &'a TimetableGraph,
    current: Option<usize>,
    ancestors: ReachabilitySearch,
    descendants: ReachabilitySearch,
}

impl<'a> DependencyOrder<'a> {
    fn new(graph: &'a TimetableGraph) -> Self {
        Self {
            graph,
            current: None,
            ancestors: ReachabilitySearch::default(),
            descendants: ReachabilitySearch::default(),
        }
    }

    fn are_ordered(&mut self, current: usize, other: usize) -> bool {
        if current == other {
            return false;
        }
        if self.current != Some(current) {
            self.current = Some(current);
            self.ancestors.clear();
            self.descendants.clear();
        }

        if self.graph.topological_position(other) < self.graph.topological_position(current) {
            self.ancestors
                .contains(self.graph, current, other, SearchDirection::Ancestors)
        } else {
            self.descendants
                .contains(self.graph, current, other, SearchDirection::Descendants)
        }
    }
}

#[derive(Default)]
struct ReachabilitySearch {
    reached: HashSet<usize>,
    pending: VecDeque<usize>,
}

impl ReachabilitySearch {
    fn clear(&mut self) {
        self.reached.clear();
        self.pending.clear();
    }

    fn contains(
        &mut self,
        graph: &TimetableGraph,
        start: usize,
        target: usize,
        direction: SearchDirection,
    ) -> bool {
        if self.reached.is_empty() {
            self.reached.insert(start);
            self.pending.push_back(start);
        }

        while !self.reached.contains(&target) {
            // Most conflicts are ordered by nearby cache-version or producer
            // dependencies. Breadth-first search checks those short paths
            // before walking a whole unrelated branch of the model.
            let Some(node) = self.pending.pop_front() else {
                return false;
            };
            let adjacent = match direction {
                SearchDirection::Ancestors => graph.nodes()[node].predecessors(),
                SearchDirection::Descendants => graph.nodes()[node].successors(),
            };
            for adjacent in adjacent {
                if self.reached.insert(*adjacent) {
                    self.pending.push_back(*adjacent);
                }
            }
        }
        true
    }
}

#[derive(Clone, Copy)]
enum SearchDirection {
    Ancestors,
    Descendants,
}

#[cfg(test)]
mod tests {
    use super::DependencyOrder;
    use crate::timetable_file::TimetableFile;

    #[test]
    fn short_dependency_path_does_not_scan_a_long_alternative() {
        use super::{ReachabilitySearch, SearchDirection};
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for index in 0..1000 {
            nodes.push(format!("{{ id: n{index}, kind: compute, program: {{ inputs: [], outputs: [], assignments: [] }} }}"));
        }
        for index in 2..999 {
            edges.push(format!(
                "{{ from: n{index}, to: n{}, kind: dependency }}",
                index + 1
            ));
        }
        edges.push("{ from: n0, to: n1, kind: dependency }".into());
        edges.push("{ from: n0, to: n2, kind: dependency }".into());
        edges.push("{ from: n1, to: n999, kind: dependency }".into());
        let source = format!("nodes: [{}]\nedges: [{}]", nodes.join(","), edges.join(","));
        let mut graph = TimetableFile::from_string(&source)
            .unwrap()
            .into_graph()
            .unwrap();
        // Put the unrelated long branch first in depth-first search order.
        graph.nodes[999].predecessors = vec![1, 998];
        graph.nodes[0].successors = vec![1, 2];
        for (start, target, direction) in [
            (999, 0, SearchDirection::Ancestors),
            (0, 999, SearchDirection::Descendants),
        ] {
            let mut search = ReachabilitySearch::default();
            assert!(search.contains(&graph, start, target, direction));
            assert!(
                search.reached.len() < 20,
                "nearby dependency explored {} nodes",
                search.reached.len()
            );
        }
    }

    #[test]
    fn searches_dependencies_lazily_in_each_direction() {
        let graph = TimetableFile::from_string(
            r"
nodes:
  - { id: initial, kind: tensor, config: { addr: 10, dtype: int8, shape: [1] } }
  - { id: compute0, kind: compute, program: { inputs: [A], outputs: [Y], assignments: [{ output: Y, axes: [i], expr: { identity: [{ read: { tensor: A, indices: [i] } }] } }] } }
  - { id: tensor0, kind: tensor, config: { addr: 0, dtype: int8, shape: [1] } }
  - { id: compute1, kind: compute, program: { inputs: [A], outputs: [Y], assignments: [{ output: Y, axes: [i], expr: { identity: [{ read: { tensor: A, indices: [i] } }] } }] } }
  - { id: tensor1, kind: tensor, config: { addr: 1, dtype: int8, shape: [1] } }
  - { id: compute2, kind: compute, program: { inputs: [A], outputs: [], assignments: [] } }
edges:
  - { from: initial, to: compute0, kind: data }
  - { from: compute0, to: tensor0, kind: data }
  - { from: tensor0, to: compute1, kind: data }
  - { from: compute1, to: tensor1, kind: data }
  - { from: tensor1, to: compute2, kind: data }
",
        )
        .unwrap()
        .into_graph()
        .unwrap();
        let mut dependencies = DependencyOrder::new(&graph);

        assert_eq!(dependencies.current, None);
        assert!(dependencies.ancestors.reached.is_empty());
        assert!(dependencies.descendants.reached.is_empty());

        assert!(dependencies.are_ordered(3, 1));
        assert_eq!(dependencies.current, Some(3));
        assert!(dependencies.ancestors.reached.contains(&1));
        assert!(dependencies.descendants.reached.is_empty());

        let num_ancestors_reached = dependencies.ancestors.reached.len();
        assert!(dependencies.are_ordered(3, 2));
        assert_eq!(dependencies.ancestors.reached.len(), num_ancestors_reached);

        assert!(dependencies.are_ordered(3, 5));
        assert!(dependencies.descendants.reached.contains(&5));

        assert!(dependencies.are_ordered(1, 5));
        assert_eq!(dependencies.current, Some(1));
        assert!(dependencies.ancestors.reached.is_empty());
        assert!(dependencies.descendants.reached.contains(&5));
    }
}
