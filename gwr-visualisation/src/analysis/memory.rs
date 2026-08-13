// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_platform::types::{MemorySection, PlatformConfig};

use super::model::{MemoryDeviceSummary, MemorySummary, TensorPeConsumption, TensorSummary};

pub(super) fn summarize_memory(
    tensors_by_id: &BTreeMap<String, TensorSummary>,
    platform: Option<&PlatformConfig>,
) -> MemorySummary {
    let min_addr = tensors_by_id.values().map(|tensor| tensor.addr).min();
    let max_addr = tensors_by_id
        .values()
        .map(|tensor| tensor.addr.saturating_add(tensor.num_bytes))
        .max();
    let platform_memories = summarize_platform_memory_allocations(tensors_by_id, platform);

    MemorySummary {
        min_addr,
        max_addr,
        total_memory_read_bytes: platform_memories.iter().fold(0_u64, |total, memory| {
            total.saturating_add(memory.read_bytes)
        }),
        total_memory_write_bytes: platform_memories.iter().fold(0_u64, |total, memory| {
            total.saturating_add(memory.write_bytes)
        }),
        platform_memories,
    }
}

fn summarize_platform_memory_allocations(
    tensors_by_id: &BTreeMap<String, TensorSummary>,
    platform: Option<&PlatformConfig>,
) -> Vec<MemoryDeviceSummary> {
    let Some(memories) = platform.and_then(|platform| platform.memories.as_ref()) else {
        return Vec::new();
    };

    let mut summaries: Vec<_> = memories
        .iter()
        .map(|memory| summarize_memory_device(memory, tensors_by_id))
        .collect();
    summaries.sort_by_key(|memory| (memory.base_addr, memory.name.clone()));
    summaries
}

fn summarize_memory_device(
    memory: &MemorySection,
    tensors_by_id: &BTreeMap<String, TensorSummary>,
) -> MemoryDeviceSummary {
    let memory_start = u128::from(memory.base_address);
    let memory_end = memory_start + u128::from(memory.capacity_bytes);
    let mut allocation_ranges = Vec::new();
    let mut read_bytes = 0_u64;
    let mut write_bytes = 0_u64;
    let mut tensor_ids = Vec::new();

    for tensor in tensors_by_id.values() {
        let tensor_start = u128::from(tensor.addr);
        let tensor_end = tensor_start + u128::from(tensor.num_bytes);
        let overlap_start = memory_start.max(tensor_start);
        let overlap_end = memory_end.min(tensor_end);
        if overlap_end <= overlap_start {
            continue;
        }

        allocation_ranges.push((overlap_start, overlap_end));
        read_bytes = read_bytes.saturating_add(traffic_in_range(
            &tensor.consumption_by_pe,
            memory_start,
            memory_end,
        ));
        write_bytes = write_bytes.saturating_add(traffic_in_range(
            &tensor.production_by_pe,
            memory_start,
            memory_end,
        ));
        tensor_ids.push(tensor.id.clone());
    }

    MemoryDeviceSummary {
        name: memory.name.clone(),
        kind: format!("{:?}", memory.kind).to_lowercase(),
        base_addr: memory.base_address,
        capacity_bytes: memory.capacity_bytes,
        allocated_bytes: union_bytes(allocation_ranges),
        read_bytes,
        write_bytes,
        tensor_count: tensor_ids.len(),
        tensors: tensor_ids,
    }
}

fn traffic_in_range(
    connections: &[TensorPeConsumption],
    range_start: u128,
    range_end: u128,
) -> u64 {
    connections
        .iter()
        .flat_map(|connection| &connection.accesses)
        .flat_map(|access| &access.ranges)
        .fold(0_u64, |total, access_range| {
            let access_start = u128::from(access_range.addr);
            let access_end = access_start + u128::from(access_range.num_bytes);
            let overlap_start = range_start.max(access_start);
            let overlap_end = range_end.min(access_end);
            let bytes = overlap_end.saturating_sub(overlap_start);
            total.saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX))
        })
}

fn union_bytes(mut ranges: Vec<(u128, u128)>) -> u64 {
    ranges.sort_unstable();
    let mut total = 0_u64;
    let mut merged: Option<(u128, u128)> = None;
    for (start, end) in ranges {
        let Some((merged_start, merged_end)) = merged else {
            merged = Some((start, end));
            continue;
        };
        if start <= merged_end {
            merged = Some((merged_start, merged_end.max(end)));
        } else {
            total =
                total.saturating_add(u64::try_from(merged_end - merged_start).unwrap_or(u64::MAX));
            merged = Some((start, end));
        }
    }
    if let Some((start, end)) = merged {
        total = total.saturating_add(u64::try_from(end - start).unwrap_or(u64::MAX));
    }
    total
}
