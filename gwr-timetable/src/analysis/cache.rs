// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Best/worst-case cache sharing estimates for roofline analysis.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use gwr_platform::builder::DEFAULT_CACHE_LINE_SIZE_BYTES;
use gwr_platform::types::PlatformConfig;

use crate::analysis::memory::BandwidthGraph;
use crate::analysis::{ComputeNodeAnalysis, TensorAccessKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheModel {
    Worst,
    Best,
}

#[derive(Clone, Debug, Default)]
pub struct CacheSharingReport {
    pub model: Option<CacheModel>,
    pub original_memory_bytes: usize,
    pub adjusted_memory_bytes: usize,
    pub memory_bytes_avoided: usize,
    pub shared_lines: Vec<SharedCacheLine>,
}

#[derive(Clone, Debug)]
pub struct SharedCacheLine {
    pub cache_name: String,
    pub memory_name: String,
    pub line_addr: u64,
    pub line_size_bytes: usize,
    pub pes: Vec<String>,
    pub node_ids: Vec<String>,
    pub read_bytes: usize,
    pub backing_memory_bytes: usize,
    pub memory_bytes_avoided: usize,
}

#[derive(Clone, Debug)]
struct AccessFragment {
    node_idx: usize,
    node_id: String,
    pe_name: String,
    cache_names: BTreeSet<String>,
    bytes: usize,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct LineKey {
    memory_name: String,
    line_addr: u64,
}

type NodeMemoryKey = (usize, String);
type AvoidedBytesByNodeMemory = Vec<(NodeMemoryKey, usize)>;
type SharedCacheLineAnalysis = (SharedCacheLine, AvoidedBytesByNodeMemory);

#[must_use]
pub fn apply_cache_model(
    compute_nodes: &[ComputeNodeAnalysis],
    graph: &BandwidthGraph,
    platform_cfg: &PlatformConfig,
    model: CacheModel,
) -> (Vec<ComputeNodeAnalysis>, CacheSharingReport) {
    let original_memory_bytes = compute_nodes
        .iter()
        .map(|node| node.bytes_by_memory.values().sum::<usize>())
        .sum();

    match model {
        CacheModel::Worst => (
            compute_nodes.to_vec(),
            CacheSharingReport {
                model: Some(CacheModel::Worst),
                original_memory_bytes,
                adjusted_memory_bytes: original_memory_bytes,
                ..CacheSharingReport::default()
            },
        ),
        CacheModel::Best => apply_best_case_cache_sharing(compute_nodes, graph, platform_cfg),
    }
}

fn apply_best_case_cache_sharing(
    compute_nodes: &[ComputeNodeAnalysis],
    graph: &BandwidthGraph,
    platform_cfg: &PlatformConfig,
) -> (Vec<ComputeNodeAnalysis>, CacheSharingReport) {
    let cache_line_sizes = cache_line_sizes(platform_cfg);
    let fragments_by_line = collect_read_fragments_by_line(compute_nodes, graph, &cache_line_sizes);
    let (avoided_by_node_memory, shared_lines) =
        find_shared_cache_lines(fragments_by_line, &cache_line_sizes);
    let adjusted_nodes = apply_avoided_memory_bytes(compute_nodes, &avoided_by_node_memory);
    let report = build_best_case_report(compute_nodes, shared_lines);

    (adjusted_nodes, report)
}

fn collect_read_fragments_by_line(
    compute_nodes: &[ComputeNodeAnalysis],
    graph: &BandwidthGraph,
    cache_line_sizes: &HashMap<String, usize>,
) -> HashMap<LineKey, Vec<AccessFragment>> {
    let mut fragments_by_line: HashMap<LineKey, Vec<AccessFragment>> = HashMap::new();

    for node in compute_nodes {
        collect_node_read_fragments(node, graph, cache_line_sizes, &mut fragments_by_line);
    }

    fragments_by_line
}

fn collect_node_read_fragments(
    node: &ComputeNodeAnalysis,
    graph: &BandwidthGraph,
    cache_line_sizes: &HashMap<String, usize>,
    fragments_by_line: &mut HashMap<LineKey, Vec<AccessFragment>>,
) {
    let Some(pe_name) = &node.pe_name else {
        return;
    };

    for access in &node.tensor_memory_accesses {
        if access.kind != TensorAccessKind::Read {
            continue;
        }

        let cache_names = graph.cache_names_on_paths(
            &format!("pe:{pe_name}"),
            &format!("mem:{}", access.memory_name),
        );
        if cache_names.is_empty() {
            continue;
        }

        let line_size = smallest_cache_line_size(&cache_names, cache_line_sizes);
        for (line_addr, bytes) in line_fragments(access.start_addr, access.end_addr, line_size) {
            fragments_by_line
                .entry(LineKey {
                    memory_name: access.memory_name.clone(),
                    line_addr,
                })
                .or_default()
                .push(AccessFragment {
                    node_idx: node.node_idx,
                    node_id: node.id.clone(),
                    pe_name: pe_name.clone(),
                    cache_names: cache_names.clone(),
                    bytes,
                });
        }
    }
}

fn smallest_cache_line_size(
    cache_names: &BTreeSet<String>,
    cache_line_sizes: &HashMap<String, usize>,
) -> usize {
    cache_names
        .iter()
        .filter_map(|cache_name| cache_line_sizes.get(cache_name))
        .copied()
        .min()
        .unwrap_or(DEFAULT_CACHE_LINE_SIZE_BYTES)
}

fn find_shared_cache_lines(
    fragments_by_line: HashMap<LineKey, Vec<AccessFragment>>,
    cache_line_sizes: &HashMap<String, usize>,
) -> (HashMap<NodeMemoryKey, usize>, Vec<SharedCacheLine>) {
    let mut avoided_by_node_memory: HashMap<NodeMemoryKey, usize> = HashMap::new();
    let mut shared_lines = Vec::new();

    for (line, mut fragments) in fragments_by_line {
        fragments.sort_by_key(|fragment| fragment.node_idx);
        let Some((shared_line, avoided_bytes)) =
            analyze_shared_cache_line(line, &fragments, cache_line_sizes)
        else {
            continue;
        };

        for (node_memory, bytes) in avoided_bytes {
            *avoided_by_node_memory.entry(node_memory).or_insert(0) += bytes;
        }
        shared_lines.push(shared_line);
    }

    shared_lines.sort_by_key(|line| std::cmp::Reverse(line.memory_bytes_avoided));
    (avoided_by_node_memory, shared_lines)
}

fn analyze_shared_cache_line(
    line: LineKey,
    fragments: &[AccessFragment],
    cache_line_sizes: &HashMap<String, usize>,
) -> Option<SharedCacheLineAnalysis> {
    let cache_name = best_shared_cache(fragments)?;
    let sharing_fragments = fragments
        .iter()
        .filter(|fragment| fragment.cache_names.contains(&cache_name))
        .collect::<Vec<_>>();
    let pes = sharing_fragments
        .iter()
        .map(|fragment| fragment.pe_name.clone())
        .collect::<BTreeSet<_>>();
    if pes.len() < 2 {
        return None;
    }

    let first_pe = &sharing_fragments[0].pe_name;
    let mut read_bytes = 0;
    let mut backing_memory_bytes = 0;
    let mut avoided_bytes = 0;
    let mut node_ids = BTreeSet::new();
    let mut avoided_by_node_memory = Vec::new();
    for fragment in sharing_fragments {
        read_bytes += fragment.bytes;
        backing_memory_bytes += fragment.bytes;
        node_ids.insert(fragment.node_id.clone());
        if &fragment.pe_name != first_pe {
            avoided_bytes += fragment.bytes;
            backing_memory_bytes -= fragment.bytes;
            avoided_by_node_memory.push((
                (fragment.node_idx, line.memory_name.clone()),
                fragment.bytes,
            ));
        }
    }

    if avoided_bytes == 0 {
        return None;
    }

    let line_size = cache_line_sizes
        .get(&cache_name)
        .copied()
        .unwrap_or(DEFAULT_CACHE_LINE_SIZE_BYTES);
    let shared_line = SharedCacheLine {
        cache_name,
        memory_name: line.memory_name,
        line_addr: line.line_addr,
        line_size_bytes: line_size,
        pes: pes.into_iter().collect(),
        node_ids: node_ids.into_iter().collect(),
        read_bytes,
        backing_memory_bytes,
        memory_bytes_avoided: avoided_bytes,
    };

    Some((shared_line, avoided_by_node_memory))
}

fn apply_avoided_memory_bytes(
    compute_nodes: &[ComputeNodeAnalysis],
    avoided_by_node_memory: &HashMap<NodeMemoryKey, usize>,
) -> Vec<ComputeNodeAnalysis> {
    let mut adjusted_nodes = compute_nodes.to_vec();

    for node in &mut adjusted_nodes {
        for (memory_name, bytes) in &mut node.bytes_by_memory {
            let avoided = avoided_by_node_memory
                .get(&(node.node_idx, memory_name.clone()))
                .copied()
                .unwrap_or(0);
            *bytes = bytes.saturating_sub(avoided);
        }
    }

    adjusted_nodes
}

fn build_best_case_report(
    compute_nodes: &[ComputeNodeAnalysis],
    shared_lines: Vec<SharedCacheLine>,
) -> CacheSharingReport {
    let original_memory_bytes = compute_nodes
        .iter()
        .map(|node| node.bytes_by_memory.values().sum::<usize>())
        .sum();
    let memory_bytes_avoided = shared_lines
        .iter()
        .map(|line| line.memory_bytes_avoided)
        .sum();
    let adjusted_memory_bytes = original_memory_bytes - memory_bytes_avoided;

    CacheSharingReport {
        model: Some(CacheModel::Best),
        original_memory_bytes,
        adjusted_memory_bytes,
        memory_bytes_avoided,
        shared_lines,
    }
}

fn best_shared_cache(fragments: &[AccessFragment]) -> Option<String> {
    let mut cache_to_pes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut cache_to_fragments: BTreeMap<String, usize> = BTreeMap::new();
    for fragment in fragments {
        for cache_name in &fragment.cache_names {
            cache_to_pes
                .entry(cache_name.clone())
                .or_default()
                .insert(fragment.pe_name.clone());
            *cache_to_fragments.entry(cache_name.clone()).or_insert(0) += 1;
        }
    }

    cache_to_pes
        .into_iter()
        .max_by_key(|(cache_name, pes)| {
            (
                pes.len(),
                cache_to_fragments.get(cache_name).copied().unwrap_or(0),
                std::cmp::Reverse(cache_name.clone()),
            )
        })
        .map(|(cache_name, _)| cache_name)
}

fn cache_line_sizes(platform_cfg: &PlatformConfig) -> HashMap<String, usize> {
    platform_cfg
        .caches
        .as_ref()
        .into_iter()
        .flatten()
        .map(|cache| {
            (
                cache.name.clone(),
                cache
                    .config
                    .line_size_bytes
                    .unwrap_or(DEFAULT_CACHE_LINE_SIZE_BYTES),
            )
        })
        .collect()
}

fn line_fragments(start_addr: u64, end_addr: u64, line_size: usize) -> Vec<(u64, usize)> {
    let line_size = line_size.max(1) as u64;
    let mut fragments = Vec::new();
    let mut line_addr = start_addr / line_size * line_size;
    while line_addr <= end_addr {
        let line_end = line_addr + line_size - 1;
        let fragment_start = start_addr.max(line_addr);
        let fragment_end = end_addr.min(line_end);
        fragments.push((line_addr, (fragment_end - fragment_start + 1) as usize));
        match line_addr.checked_add(line_size) {
            Some(next) => line_addr = next,
            None => break,
        }
    }
    fragments
}
