// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Stable SRAM slot allocation for values local to one output tile.

use std::collections::{BTreeMap, BTreeSet};

use gwr_engine::sim_error;
use gwr_engine::types::SimError;

use super::syntax::ExprSpec;
use super::tile::RegionTile;
use super::validate::ValidatedRegion;

pub fn allocate_temporaries(
    region: &ValidatedRegion,
    tile: &RegionTile,
) -> Result<LocalAllocation, SimError> {
    let mut layers = BTreeMap::new();
    let mut done = BTreeSet::new();
    let mut remaining = region.assignment_order.clone();
    let mut layer = 0usize;
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .copied()
            .filter(|index| {
                let mut dependencies = BTreeSet::new();
                reads(&region.program.assignments[*index].expr, &mut dependencies);
                dependencies
                    .iter()
                    .all(|name| region.program.inputs.contains(name) || done.contains(name))
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
    let mut last_use = BTreeMap::new();
    for (index, assignment) in region.program.assignments.iter().enumerate() {
        let mut dependencies = BTreeSet::new();
        reads(&assignment.expr, &mut dependencies);
        for name in dependencies {
            last_use
                .entry(name)
                .and_modify(|last: &mut usize| *last = (*last).max(layers[&index]))
                .or_insert(layers[&index]);
        }
    }
    let mut slots = Vec::<LocalSlot>::new();
    let mut occupied_until = Vec::<usize>::new();
    for index in &region.assignment_order {
        let assignment = &region.program.assignments[*index];
        let elements = assignment.axes.iter().try_fold(1usize, |count, axis| {
            let len = tile
                .output_intervals
                .get(axis)
                .map_or(region.extent(axis).unwrap(), |range| range.len());
            count
                .checked_mul(len)
                .ok_or_else(|| SimError("local value element count overflows".into()))
        })?;
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
            let bytes = align8(
                elements
                    .checked_mul(temp.dtype.num_bits())
                    .ok_or_else(|| SimError("local value size overflows".into()))?
                    .div_ceil(8),
            )?;
            let produced = layers[index];
            let last = last_use.get(name).copied().unwrap_or(produced);
            let available = slots
                .iter()
                .enumerate()
                .filter(|(slot, entry)| {
                    occupied_until[*slot] < produced && entry.capacity_bytes >= bytes
                })
                .min_by_key(|(_, entry)| entry.capacity_bytes)
                .map(|(slot, _)| slot);
            if let Some(slot) = available {
                slots[slot].values.push(name.clone());
                occupied_until[slot] = last;
            } else {
                slots.push(LocalSlot {
                    capacity_bytes: bytes,
                    values: vec![name.clone()],
                });
                occupied_until.push(last);
            }
        }
    }
    let arena_bytes = slots.iter().try_fold(0usize, |sum, slot| {
        sum.checked_add(slot.capacity_bytes)
            .ok_or_else(|| SimError("local arena size overflows".into()))
    })?;
    Ok(LocalAllocation { slots, arena_bytes })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalAllocation {
    pub slots: Vec<LocalSlot>,
    pub arena_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalSlot {
    pub capacity_bytes: usize,
    pub values: Vec<String>,
}

fn align8(bytes: usize) -> Result<usize, SimError> {
    bytes
        .checked_add(7)
        .map(|value| value & !7)
        .ok_or_else(|| SimError("local slot alignment overflows".into()))
}

fn reads(expr: &ExprSpec, names: &mut BTreeSet<String>) {
    match expr {
        ExprSpec::Read { tensor, .. } => {
            names.insert(tensor.clone());
        }
        ExprSpec::Call { args, .. } | ExprSpec::MatrixMac { args, .. } => {
            for arg in args {
                reads(arg, names);
            }
        }
        ExprSpec::Reduce { value, .. } => reads(value, names),
    }
}
