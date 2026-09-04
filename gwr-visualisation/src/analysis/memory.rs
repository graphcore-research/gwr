// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_engine::types::SimError;
use gwr_platform::types::{MemorySection, PlatformConfig};
use gwr_timetable::{ComputeTensorDirection, TimetableGraph};

use super::{add_u64, u64_from_u128, u64_from_usize};
use crate::address::{AddressRange, range_union_length};
use crate::model::{MemoryDeviceSummary, MemorySummary, TensorSummary};

pub(super) fn summarize_memory(
    graph: &TimetableGraph,
    tensors: &BTreeMap<usize, TensorSummary>,
    platform: Option<&PlatformConfig>,
) -> Result<MemorySummary, SimError> {
    let min_addr = tensors.values().map(|tensor| tensor.addr).min();
    let max_addr = tensors
        .values()
        .map(|tensor| AddressRange::new(tensor.addr, tensor.num_bytes).end)
        .max();
    let platform_memories = summarize_platform_memories(graph, tensors, platform)?;
    let mut total_memory_read_bytes = 0;
    let mut total_memory_write_bytes = 0;
    for memory in &platform_memories {
        add_u64(
            &mut total_memory_read_bytes,
            memory.read_bytes,
            "Report memory read byte total",
        )?;
        add_u64(
            &mut total_memory_write_bytes,
            memory.write_bytes,
            "Report memory write byte total",
        )?;
    }

    Ok(MemorySummary {
        min_addr,
        max_addr,
        total_memory_read_bytes,
        total_memory_write_bytes,
        platform_memories,
    })
}

fn summarize_platform_memories(
    graph: &TimetableGraph,
    tensors: &BTreeMap<usize, TensorSummary>,
    platform: Option<&PlatformConfig>,
) -> Result<Vec<MemoryDeviceSummary>, SimError> {
    let Some(memories) = platform.and_then(|platform| platform.memories.as_ref()) else {
        return Ok(Vec::new());
    };

    let mut summaries = memories
        .iter()
        .map(|memory| summarize_memory_device(memory, graph, tensors))
        .collect::<Result<Vec<_>, _>>()?;
    summaries.sort_by_key(|memory| (memory.base_addr, memory.name.clone()));
    Ok(summaries)
}

fn summarize_memory_device(
    memory: &MemorySection,
    graph: &TimetableGraph,
    tensors: &BTreeMap<usize, TensorSummary>,
) -> Result<MemoryDeviceSummary, SimError> {
    let capacity_bytes = memory.config.capacity_bytes;
    let memory_range = AddressRange::new(memory.base_address, capacity_bytes);
    let mut allocation_ranges = Vec::new();
    let mut tensor_ids = Vec::new();
    for (node_index, tensor) in tensors {
        let tensor_range = AddressRange::new(tensor.addr, tensor.num_bytes);
        if let Some(overlap) = memory_range.intersection(tensor_range) {
            allocation_ranges.push(overlap);
            tensor_ids.push((*node_index, tensor.id.clone()));
        }
    }

    let mut read_bytes = 0;
    let mut write_bytes = 0;
    for connection in graph
        .edges()
        .iter()
        .filter_map(|edge| edge.tensor_connection())
    {
        if !tensors.contains_key(&connection.tensor_node()) {
            continue;
        }
        let bytes = u64_from_usize(
            connection
                .view()
                .num_access_bytes_in(memory_range.start..memory_range.end),
            "memory-attributed tensor transfer",
        )?;
        if connection.direction() == ComputeTensorDirection::Input {
            add_u64(&mut read_bytes, bytes, "Memory read byte total")?;
        } else {
            add_u64(&mut write_bytes, bytes, "Memory write byte total")?;
        }
    }

    tensor_ids.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(MemoryDeviceSummary {
        name: memory.name.clone(),
        kind: format!("{:?}", memory.kind).to_lowercase(),
        base_addr: memory.base_address,
        capacity_bytes,
        allocated_bytes: u64_from_u128(
            range_union_length(allocation_ranges),
            "memory allocation total",
        )?,
        read_bytes,
        write_bytes,
        tensor_count: u64_from_usize(tensor_ids.len(), "memory tensor count")?,
        tensors: tensor_ids.into_iter().map(|(_, id)| id).collect(),
    })
}
