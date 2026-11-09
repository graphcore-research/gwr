// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Caller-qualified logical local service for exactly one source scalar recipe.
//! Existing checked debt mechanics are reused with a SEPARATE PE instance.
//! Canceled waits own no local byte worker; their numeric debt is not SRAM
//! custody.
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use gwr_engine::types::{SimError, SimResult};

pub(crate) use super::boundary_payload::BoundaryPayloadDirection as LocalPayloadDirection;
#[cfg(test)]
pub(crate) use super::boundary_payload::BoundaryPayloadService as LocalPayloadService;
use super::boundary_payload::BoundaryPayloadSnapshot;
use super::program::plan::{LocalAllocation, allocate_temporaries};
use super::program::syntax::{ExprSpec, LocalRoundPolicy, ProgramSpec};
use super::program::tile::{RegionTile, plan_tiles};
use super::program::validate::{RegionBindings, ValidatedRegion, validate_program};
use super::task::ProgramTaskConfig;
use super::tensors::HasShape;
use super::tensors::dtype::DataType;

/// Logical accepted/completed/canceled temporary bytes, separate from PE/HBM
/// traffic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct LocalPayloadSnapshot {
    pub admitted_read_bytes: usize,
    pub admitted_write_bytes: usize,
    pub completed_read_bytes: usize,
    pub completed_write_bytes: usize,
    pub canceled_read_bytes: usize,
    pub canceled_write_bytes: usize,
}
impl From<BoundaryPayloadSnapshot> for LocalPayloadSnapshot {
    fn from(v: BoundaryPayloadSnapshot) -> Self {
        Self {
            admitted_read_bytes: v.admitted_read_bytes,
            admitted_write_bytes: v.admitted_write_bytes,
            completed_read_bytes: v.completed_read_bytes,
            completed_write_bytes: v.completed_write_bytes,
            canceled_read_bytes: v.canceled_read_bytes,
            canceled_write_bytes: v.canceled_write_bytes,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssignmentFootprint {
    pub read_ranges: Vec<Range<usize>>,
    pub write_ranges: Vec<Range<usize>>,
    pub read_bytes: usize,
    pub write_bytes: usize,
}

fn error(message: &str) -> SimError {
    SimError(format!("local round policy: {message}"))
}

pub(crate) fn canonical_program() -> Result<ProgramSpec, SimError> {
    yaml_serde::from_str(include_str!("v41_round_scale_scalar.json"))
        .map_err(|e| SimError(format!("local round canonical source: {e}")))
}

/// Before activation, demand the exact bounded source AST and actual typed
/// scalar views. Ordinary None never enters this validator and retains its old
/// behavior.
pub(crate) fn validate_frontier(config: &ProgramTaskConfig, service_present: bool) -> SimResult {
    let Some(LocalRoundPolicy::V41RoundScaleScalar) = config.program.local_round_policy else {
        return Ok(());
    };
    if !service_present {
        return Err(error("explicit local payload service required"));
    }
    if config.tiling.is_some() {
        return Err(error("explicit tiling is unsupported for scalar b=1"));
    }
    let mut plain = config.program.clone();
    plain.local_round_policy = None;
    if plain != canonical_program()? {
        return Err(error("canonical source program required"));
    }
    let types = [
        DataType::Fp32,
        DataType::Uint32,
        DataType::Uint32,
        DataType::Uint32,
        DataType::Uint32,
        DataType::Int32,
        DataType::Int32,
        DataType::Int32,
        DataType::Int32,
    ];
    if config.inputs.len() != types.len() || config.outputs.len() != 1 {
        return Err(error("exact source bindings required"));
    }
    for (index, (view, dtype)) in config.inputs.iter().zip(types).enumerate() {
        let shape = if index == 0 { vec![1] } else { vec![] };
        if view.dtype() != &dtype || view.shape().dims() != shape.as_slice() {
            return Err(error("source input dtype/scalar shape differs"));
        }
    }
    if config.outputs[0].dtype() != &DataType::Fp32 || config.outputs[0].shape().dims() != [1] {
        return Err(error("source output dtype/shape differs"));
    }
    Ok(())
}

pub(crate) fn preflight(
    config: &ProgramTaskConfig,
    service_present: bool,
    sram_bytes: usize,
) -> SimResult {
    if config.program.local_round_policy.is_none() {
        return Ok(());
    }
    validate_frontier(config, service_present)?;
    let region = validate_program(
        &config.program,
        &RegionBindings {
            inputs: config.inputs.clone(),
            outputs: config.outputs.clone(),
        },
    )?;
    let schedule = plan_tiles(&region, None)?;
    let tile = schedule
        .tiles()
        .next()
        .ok_or_else(|| error("scalar tile absent"))?;
    let allocation = allocate_temporaries(&region, &tile)?;
    let prefix = boundary_arena_bytes(config)?;
    footprints(&region, &tile, &allocation, prefix)?;
    let full = prefix
        .checked_add(allocation.arena_bytes)
        .ok_or_else(|| error("arena overflows"))?;
    if full > sram_bytes {
        return Err(error("source scalar tile exceeds SRAM capacity"));
    }
    Ok(())
}

pub(crate) fn boundary_arena_bytes(config: &ProgramTaskConfig) -> Result<usize, SimError> {
    config
        .inputs
        .iter()
        .chain(&config.outputs)
        .try_fold(0usize, |sum, view| {
            let n = view.num_packed_bytes();
            let aligned = n
                .checked_add(7)
                .ok_or_else(|| error("boundary alignment overflows"))?
                & !7;
            sum.checked_add(aligned)
                .ok_or_else(|| error("boundary arena overflows"))
        })
}

/// Union logical local byte addresses within an assignment; repeated
/// assignments remain separately charged. Addresses refer to the existing
/// LocalAllocation.
pub(crate) fn footprints(
    region: &ValidatedRegion,
    tile: &RegionTile,
    allocation: &LocalAllocation,
    base: usize,
) -> Result<Vec<AssignmentFootprint>, SimError> {
    if region.extent("b") != Some(1) || tile.output_intervals.get("b") != Some(&(0..1)) {
        return Err(error("only source scalar b=1 tile is admitted"));
    }
    let mut names = BTreeMap::new();
    let mut offset = base;
    for slot in &allocation.slots {
        let end = offset
            .checked_add(slot.capacity_bytes)
            .ok_or_else(|| error("slot endpoint overflows"))?;
        for name in &slot.values {
            if names.insert(name.clone(), (offset, end)).is_some() {
                return Err(error("duplicate local slot value"));
            }
        }
        offset = end;
    }
    if offset.checked_sub(base) != Some(allocation.arena_bytes)
        || names.len() != region.program.temporaries.len()
    {
        return Err(error("actual allocation roster differs"));
    }
    let local_range = |name: &str| -> Result<Range<usize>, SimError> {
        let (start, slot_end) = names
            .get(name)
            .copied()
            .ok_or_else(|| error("local value missing from allocation"))?;
        let bits = region
            .value(name)
            .ok_or_else(|| error("typed local value missing"))?
            .dtype
            .num_bits();
        let bytes = bits
            .checked_add(7)
            .ok_or_else(|| error("packed local size overflows"))?
            / 8;
        let end = start
            .checked_add(bytes)
            .ok_or_else(|| error("local endpoint overflows"))?;
        if end > slot_end {
            return Err(error("logical payload exceeds actual slot"));
        }
        Ok(start..end)
    };
    region
        .program
        .assignments
        .iter()
        .map(|assignment| {
            let mut read_names = BTreeSet::new();
            collect_reads(&assignment.expr, &mut read_names)?;
            let reads = read_names
                .iter()
                .filter(|name| region.program.temporaries.contains_key(*name))
                .map(|name| local_range(name))
                .collect::<Result<Vec<_>, _>>()?;
            let writes = assignment
                .output
                .iter()
                .filter(|name| region.program.temporaries.contains_key(*name))
                .map(|name| local_range(name))
                .collect::<Result<Vec<_>, _>>()?;
            let read_ranges = union(reads);
            let write_ranges = union(writes);
            let count = |ranges: &[Range<usize>]| {
                ranges.iter().try_fold(0usize, |sum, r| {
                    sum.checked_add(r.len())
                        .ok_or_else(|| error("logical local count overflows"))
                })
            };
            Ok(AssignmentFootprint {
                read_bytes: count(&read_ranges)?,
                write_bytes: count(&write_ranges)?,
                read_ranges,
                write_ranges,
            })
        })
        .collect()
}
fn collect_reads(expr: &ExprSpec, names: &mut BTreeSet<String>) -> SimResult {
    match expr {
        ExprSpec::Read { tensor, .. } => {
            names.insert(tensor.clone());
        }
        ExprSpec::Call { args, .. } => {
            for arg in args {
                collect_reads(arg, names)?;
            }
        }
        _ => return Err(error("only canonical scalar read/call tree is supported")),
    }
    Ok(())
}
fn union(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut result: Vec<Range<usize>> = vec![];
    for r in ranges {
        if let Some(last) = result.last_mut() {
            if r.start <= last.end {
                last.end = last.end.max(r.end);
                continue;
            }
        }
        result.push(r);
    }
    result
}

#[cfg(test)]
#[path = "local_payload_tests.rs"]
mod tests;
