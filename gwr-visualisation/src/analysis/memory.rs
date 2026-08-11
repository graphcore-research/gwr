// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_platform::types::{MemorySection, PlatformConfig};

use super::model::{MemoryDeviceSummary, MemorySummary, TensorSummary};

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
    let mut allocated_bytes = 0_u64;
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

        let overlap_bytes = u64::try_from(overlap_end - overlap_start).unwrap_or(u64::MAX);
        allocated_bytes = allocated_bytes.saturating_add(overlap_bytes);
        read_bytes = read_bytes.saturating_add(scale_tensor_bytes(
            tensor_memory_read_bytes(tensor),
            overlap_bytes,
            tensor.num_bytes,
        ));
        write_bytes = write_bytes.saturating_add(scale_tensor_bytes(
            tensor_memory_write_bytes(tensor),
            overlap_bytes,
            tensor.num_bytes,
        ));
        tensor_ids.push(tensor.id.clone());
    }

    MemoryDeviceSummary {
        name: memory.name.clone(),
        kind: format!("{:?}", memory.kind).to_lowercase(),
        base_addr: memory.base_address,
        capacity_bytes: memory.capacity_bytes,
        allocated_bytes,
        read_bytes,
        write_bytes,
        tensor_count: tensor_ids.len(),
        tensors: tensor_ids,
    }
}

fn tensor_memory_read_bytes(tensor: &TensorSummary) -> u64 {
    tensor
        .consumption_by_pe
        .iter()
        .fold(0_u64, |total, connection| {
            total.saturating_add(connection.bytes)
        })
}

fn tensor_memory_write_bytes(tensor: &TensorSummary) -> u64 {
    tensor
        .production_by_pe
        .iter()
        .fold(0_u64, |total, connection| {
            total.saturating_add(connection.bytes)
        })
}

fn scale_tensor_bytes(bytes: u64, overlap_bytes: u64, tensor_bytes: u64) -> u64 {
    if tensor_bytes == 0 {
        return 0;
    }
    ((bytes as u128 * overlap_bytes as u128) / tensor_bytes as u128) as u64
}
