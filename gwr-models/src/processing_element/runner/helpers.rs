// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet};

use gwr_engine::types::SimError;

use crate::processing_element::program::plan::LocalAllocation;
use crate::processing_element::program::prepared::GroupPlan;

pub(super) fn build_assignment_dependencies(
    plan: GroupPlan<'_>,
    allocation: &LocalAllocation,
) -> Result<BTreeMap<usize, BTreeSet<usize>>, SimError> {
    let region = plan.region();
    let mut dependencies = plan
        .assignments
        .iter()
        .map(|&index| {
            (
                index,
                region.dependencies[index]
                    .iter()
                    .copied()
                    .filter(|i| plan.assignments.contains(i))
                    .collect::<BTreeSet<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for slot in &allocation.slots {
        for pair in slot.values.windows(2) {
            let next = region.producers[&pair[1]];
            let required = dependencies.get_mut(&next).unwrap();
            required.insert(region.producers[&pair[0]]);
            if let Some(consumers) = region.consumers.get(&pair[0]) {
                required.extend(consumers.iter().filter(|i| plan.assignments.contains(i)));
            }
        }
    }
    Ok(dependencies)
}
