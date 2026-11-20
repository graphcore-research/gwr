// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet};

use crate::processing_element::program::plan::LocalAllocation;
use crate::processing_element::program::prepared::GroupPlan;

pub(super) fn build_slot_dependencies(
    plan: GroupPlan<'_>,
    allocation: &LocalAllocation,
) -> BTreeMap<usize, BTreeSet<usize>> {
    let region = plan.region();
    let mut dependencies = BTreeMap::<usize, BTreeSet<usize>>::new();
    for slot in &allocation.slots {
        for pair in slot.values.windows(2) {
            let next = region.producers[&pair[1]];
            let required = dependencies.entry(next).or_default();
            required.insert(region.producers[&pair[0]]);
            if let Some(consumers) = region.consumers.get(&pair[0]) {
                required.extend(
                    consumers
                        .iter()
                        .filter(|i| plan.group.dependencies.contains_key(i)),
                );
            }
        }
    }
    dependencies
}
