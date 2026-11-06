// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeSet;

use gwr_engine::types::{SimError, SimResult};

use super::schedule::{ScopedSchedule, plan_schedule};
use super::syntax::{ProgramSpec, TileSpec};
use super::tile::{RegionTile, TileSchedule, plan_tiles};
use super::validate::{ProgramFacts, RegionBindings, TensorSpec, validate_program};

/// Immutable, owned tensor program with one canonical lazy schedule.
///
/// Construction binds serialized syntax and takes ownership of the tensor
/// views. The result does not borrow the caller's mutable syntax or bindings.
/// It can be shared for hardware-independent analysis or prepared against a
/// specific PE.
#[derive(Debug)]
pub struct PreparedProgram {
    pub(in crate::processing_element) facts: ProgramFacts,
    pub(in crate::processing_element) schedule: Schedule,
    pub(in crate::processing_element) retained: BTreeSet<String>,
    pub(in crate::processing_element) reduction_steps: std::collections::BTreeMap<String, usize>,
}

#[derive(Debug)]
pub(in crate::processing_element) enum Schedule {
    Flat(TileSchedule),
    Scoped(ScopedSchedule),
}

impl PreparedProgram {
    /// Validate serialized syntax once and take ownership of its tensor
    /// bindings. No hardware resources are used. Later changes to `spec` cannot
    /// alter the bound program.
    pub fn new(
        spec: &ProgramSpec,
        bindings: RegionBindings,
        tiling: Option<&TileSpec>,
    ) -> Result<Self, SimError> {
        let facts = validate_program(spec, bindings)?;
        Self::from_facts(facts, tiling)
    }
    pub(in crate::processing_element) fn from_facts(
        facts: ProgramFacts,
        tiling: Option<&TileSpec>,
    ) -> Result<Self, SimError> {
        let schedule = match plan_schedule(&facts, tiling)? {
            Some(scope) => Schedule::Scoped(scope),
            None => Schedule::Flat(plan_tiles(&facts, tiling)?),
        };
        let retained = match &schedule {
            Schedule::Scoped(scope) => super::schedule::collect_retained_values(scope),
            Schedule::Flat(_) => BTreeSet::new(),
        };
        let reduction_steps = facts
            .extents
            .iter()
            .map(|(name, extent)| {
                (
                    name.clone(),
                    tiling
                        .and_then(|t| t.sizes.get(name))
                        .copied()
                        .unwrap_or(*extent),
                )
            })
            .collect();
        Ok(Self {
            facts,
            schedule,
            retained,
            reduction_steps,
        })
    }
    /// Number of executable assignments in the program.
    #[must_use]
    pub fn assignment_count(&self) -> usize {
        self.facts.program.assignments.len()
    }
    /// Resolved value metadata.
    #[must_use]
    pub fn value(&self, name: &str) -> Option<&TensorSpec> {
        self.facts.value(name)
    }
    /// Resolved axis extents.
    #[must_use]
    pub fn extents(&self) -> &std::collections::BTreeMap<String, usize> {
        &self.facts.extents
    }
    /// Resolved extent of one axis.
    #[must_use]
    pub fn extent(&self, axis: &str) -> Option<usize> {
        self.facts.extent(axis)
    }
    pub(in crate::processing_element) fn group<'a>(
        &'a self,
        assignments: &'a [usize],
    ) -> GroupPlan<'a> {
        GroupPlan {
            program: self,
            assignments,
        }
    }
    pub(in crate::processing_element) fn visit_groups(
        &self,
        visit: &mut impl FnMut(GroupPlan<'_>, &RegionTile, usize) -> SimResult,
    ) -> SimResult {
        match &self.schedule {
            Schedule::Flat(schedule) => {
                for tile in schedule.tiles() {
                    visit(self.group(&self.facts.assignment_order), &tile, 0)?;
                }
                Ok(())
            }
            Schedule::Scoped(scope) => {
                super::schedule::visit_groups(scope, self, &mut |indices, tile, local| {
                    visit(self.group(indices), tile, local)
                })
            }
        }
    }
}

/// A borrowing projection retaining original assignment identities.
#[derive(Clone, Copy)]
pub(in crate::processing_element) struct GroupPlan<'a> {
    pub program: &'a PreparedProgram,
    pub assignments: &'a [usize],
}
impl<'a> GroupPlan<'a> {
    pub fn region(self) -> &'a ProgramFacts {
        &self.program.facts
    }
    pub fn assignments(self) -> impl Iterator<Item = &'a super::executable::Assignment> {
        self.assignments
            .iter()
            .map(move |&i| &self.region().program.assignments[i])
    }
    pub fn inputs(
        self,
    ) -> impl Iterator<Item = (&'a String, &'a super::super::tensors::TensorView)> {
        self.region()
            .program
            .inputs
            .iter()
            .zip(&self.region().bindings.inputs)
    }
    pub fn outputs(
        self,
    ) -> impl Iterator<Item = (&'a String, &'a super::super::tensors::TensorView)> {
        self.region()
            .program
            .outputs
            .iter()
            .zip(&self.region().bindings.outputs)
            .filter(move |(name, _)| self.assignments.contains(&self.region().producers[*name]))
    }
}
