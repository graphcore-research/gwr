// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeSet;

use gwr_engine::types::{SimError, SimResult};

use super::group::GroupMetadata;
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
    declared_tile_sizes: std::collections::BTreeMap<String, usize>,
    pub(in crate::processing_element) facts: ProgramFacts,
    pub(in crate::processing_element) schedule: Schedule,
    pub(in crate::processing_element) retained: BTreeSet<String>,
    pub(in crate::processing_element) reduction_steps: std::collections::BTreeMap<String, usize>,
}

#[derive(Debug)]
pub(in crate::processing_element) enum Schedule {
    Flat {
        tiles: TileSchedule,
        group: Box<GroupMetadata>,
    },
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
            None => {
                let mut group = GroupMetadata::new(&facts, facts.assignment_order.clone());
                group.issue_order.sort_unstable();
                Schedule::Flat {
                    tiles: plan_tiles(&facts, tiling)?,
                    group: Box::new(group),
                }
            }
        };
        let retained = match &schedule {
            Schedule::Scoped(scope) => super::schedule::collect_retained_values(scope),
            Schedule::Flat { .. } => BTreeSet::new(),
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
            declared_tile_sizes: tiling.map(|spec| spec.sizes.clone()).unwrap_or_default(),
            facts,
            schedule,
            retained,
            reduction_steps,
        })
    }
    /// Explicitly requested tile sizes, in axis name order.
    #[must_use]
    pub fn declared_tile_sizes(&self) -> &std::collections::BTreeMap<String, usize> {
        &self.declared_tile_sizes
    }

    /// Maximum tile size for each resolved axis, including implicit full
    /// extents. Final tiles may be smaller. This reads retained steps without
    /// visiting tiles.
    pub fn effective_tile_sizes(&self) -> impl Iterator<Item = (&str, usize)> {
        self.reduction_steps
            .iter()
            .map(|(axis, step)| (axis.as_str(), (*step).min(self.facts.extents[axis])))
    }

    /// Declared input names in dense binding order.
    #[must_use]
    pub fn input_names(&self) -> &[String] {
        &self.facts.program.inputs
    }
    /// Declared output names in dense binding order.
    #[must_use]
    pub fn output_names(&self) -> &[String] {
        &self.facts.program.outputs
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
        group: &'a GroupMetadata,
    ) -> GroupPlan<'a> {
        GroupPlan {
            program: self,
            group,
        }
    }
    pub(in crate::processing_element) fn visit_groups(
        &self,
        visit: &mut impl FnMut(GroupPlan<'_>, &RegionTile, usize) -> SimResult,
    ) -> SimResult {
        match &self.schedule {
            Schedule::Flat { tiles, group } => {
                for tile in tiles.tiles() {
                    visit(self.group(group), &tile, 0)?;
                }
                Ok(())
            }
            Schedule::Scoped(scope) => {
                super::schedule::visit_groups(scope, self, &mut |group, tile, local| {
                    visit(self.group(group), tile, local)
                })
            }
        }
    }
}

/// A borrowing projection retaining original assignment identities.
#[derive(Clone, Copy)]
pub(in crate::processing_element) struct GroupPlan<'a> {
    pub program: &'a PreparedProgram,
    pub group: &'a GroupMetadata,
}
impl<'a> GroupPlan<'a> {
    pub fn region(self) -> &'a ProgramFacts {
        &self.program.facts
    }
    pub fn assignments(self) -> impl Iterator<Item = &'a super::executable::Assignment> {
        self.group
            .assignments
            .iter()
            .map(move |&i| &self.region().program.assignments[i])
    }
}
