// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Stable SRAM slot allocation for values local to one output tile.

use std::collections::BTreeMap;

use gwr_engine::types::SimError;

use super::prepared::GroupPlan;
use super::storage::align_sram_slot_bytes;
use super::tile::RegionTile;
use super::validate::ProgramFacts;

pub(in crate::processing_element) fn allocate_temporaries(
    plan: GroupPlan<'_>,
    tile: &RegionTile,
) -> Result<LocalAllocation, SimError> {
    let region = plan.region();
    let layers = calculate_assignment_layers(plan)?;
    let last_use = last_consumer_layer_by_value(plan, &layers);
    let mut slots = Vec::<LocalSlot>::new();
    let mut occupied_until = Vec::<usize>::new();
    for index in plan.assignments {
        let assignment = &region.program.assignments[*index];
        let elements = count_assignment_tile_elements(&assignment.axes, region, tile)?;
        for name in [assignment.output(), assignment.indices_output()]
            .into_iter()
            .flatten()
        {
            if plan.program.retained.contains(name) {
                continue;
            }
            let Some(temp) = region.program.temporaries.get(name) else {
                continue;
            };
            let bytes = align_sram_slot_bytes(
                elements
                    .checked_mul(temp.dtype.num_bits())
                    .ok_or_else(|| SimError("local value size overflows".into()))?
                    .div_ceil(8),
            )?;
            let produced = layers[index];
            let last = last_use.get(name).copied().unwrap_or(produced);
            assign_local_slot(&mut slots, &mut occupied_until, name, bytes, produced, last);
        }
    }
    let arena_bytes = slots.iter().try_fold(0usize, |sum, slot| {
        sum.checked_add(slot.capacity_bytes)
            .ok_or_else(|| SimError("local arena size overflows".into()))
    })?;
    Ok(LocalAllocation { slots, arena_bytes })
}

fn last_consumer_layer_by_value(
    plan: GroupPlan<'_>,
    layers: &BTreeMap<usize, usize>,
) -> BTreeMap<String, usize> {
    plan.region()
        .consumers
        .iter()
        .filter_map(|(name, consumers)| {
            consumers
                .iter()
                .filter_map(|i| layers.get(i))
                .max()
                .map(|last| (name.clone(), *last))
        })
        .collect()
}

fn count_assignment_tile_elements(
    axes: &[String],
    region: &ProgramFacts,
    tile: &RegionTile,
) -> Result<usize, SimError> {
    axes.iter().try_fold(1usize, |count, axis| {
        let len = tile
            .output_intervals
            .get(axis)
            .map_or(region.extent(axis).unwrap(), |range| range.len());
        count
            .checked_mul(len)
            .ok_or_else(|| SimError("local value element count overflows".into()))
    })
}

fn assign_local_slot(
    slots: &mut Vec<LocalSlot>,
    occupied_until: &mut Vec<usize>,
    name: &str,
    bytes: usize,
    produced: usize,
    last: usize,
) {
    let available = slots
        .iter()
        .enumerate()
        .filter(|(slot, entry)| occupied_until[*slot] < produced && entry.capacity_bytes >= bytes)
        .min_by_key(|(_, entry)| entry.capacity_bytes)
        .map(|(slot, _)| slot);
    if let Some(slot) = available {
        slots[slot].values.push(name.to_owned());
        occupied_until[slot] = last;
    } else {
        slots.push(LocalSlot {
            capacity_bytes: bytes,
            values: vec![name.to_owned()],
        });
        occupied_until.push(last);
    }
}

/// Dependencies without a producer in this group belong to an enclosing scope.
fn calculate_assignment_layers(plan: GroupPlan<'_>) -> Result<BTreeMap<usize, usize>, SimError> {
    let mut layers = BTreeMap::new();
    for &index in plan.assignments {
        let layer = plan.region().dependencies[index]
            .iter()
            .filter_map(|dependency| layers.get(dependency).copied())
            .max()
            .map_or(0, |layer: usize| layer + 1);
        layers.insert(index, layer);
    }
    Ok(layers)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::processing_element) struct LocalAllocation {
    pub(in crate::processing_element) slots: Vec<LocalSlot>,
    pub(in crate::processing_element) arena_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::processing_element) struct LocalSlot {
    pub(in crate::processing_element) capacity_bytes: usize,
    pub(in crate::processing_element) values: Vec<String>,
}
