// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Stable SRAM slot allocation for values local to one output tile.

use std::collections::{BTreeMap, BTreeSet};

use gwr_engine::sim_error;
use gwr_engine::types::SimError;

use super::storage::align_sram_slot_bytes;
use super::tile::RegionTile;
use super::validate::ValidatedRegion;

pub(in crate::processing_element) fn allocate_temporaries(
    region: &ValidatedRegion,
    tile: &RegionTile,
) -> Result<LocalAllocation, SimError> {
    let layers = calculate_assignment_layers(region)?;
    let last_use = last_consumer_layer_by_value(region, &layers);
    let mut slots = Vec::<LocalSlot>::new();
    let mut occupied_until = Vec::<usize>::new();
    for index in &region.assignment_order {
        let assignment = &region.program.assignments[*index];
        let elements = count_assignment_tile_elements(&assignment.axes, region, tile)?;
        for name in [
            assignment.output.as_ref(),
            assignment.indices_output.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
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
    region: &ValidatedRegion,
    layers: &BTreeMap<usize, usize>,
) -> BTreeMap<String, usize> {
    let mut last_use = BTreeMap::new();
    for (index, assignment) in region.program.assignments.iter().enumerate() {
        let dependencies = assignment.expr.collect_read_tensor_names();
        for name in dependencies {
            last_use
                .entry(name)
                .and_modify(|last: &mut usize| *last = (*last).max(layers[&index]))
                .or_insert(layers[&index]);
        }
    }
    last_use
}

fn count_assignment_tile_elements(
    axes: &[String],
    region: &ValidatedRegion,
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
fn calculate_assignment_layers(
    region: &ValidatedRegion,
) -> Result<BTreeMap<usize, usize>, SimError> {
    let produced = region
        .program
        .assignments
        .iter()
        .flat_map(|assignment| {
            assignment
                .output
                .iter()
                .chain(assignment.indices_output.iter())
                .cloned()
        })
        .collect::<BTreeSet<_>>();
    let mut layers = BTreeMap::new();
    let mut done = BTreeSet::new();
    let mut remaining = region.assignment_order.clone();
    let mut layer = 0usize;
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .copied()
            .filter(|index| {
                region.program.assignments[*index]
                    .expr
                    .iter_read_tensor_names()
                    .all(|name| !produced.contains(name) || done.contains(name))
            })
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return sim_error!("local assignments cannot be scheduled");
        }
        for index in &ready {
            let assignment = &region.program.assignments[*index];
            layers.insert(*index, layer);
            done.insert(assignment.primary_result().clone());
            if let Some(index_output) = &assignment.indices_output {
                done.insert(index_output.clone());
            }
        }
        remaining.retain(|index| !ready.contains(index));
        layer = layer
            .checked_add(1)
            .ok_or_else(|| SimError("assignment layer overflows".into()))?;
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
