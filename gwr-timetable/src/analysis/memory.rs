// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BinaryHeap, HashMap};

use gwr_engine::time::clock::Clock;
use gwr_platform::Platform;

use crate::analysis::pe::PeActivity;
use crate::analysis::{bytes_per_tick_to_gb_per_s, format_bytes, ticks_to_ns};

#[derive(Clone, Debug)]
pub struct MemoryContentionWindow {
    pub start_ticks: f64,
    pub end_ticks: f64,
    pub requested_fraction: f64,
    pub active_node_indices: Vec<usize>,
}

impl MemoryContentionWindow {
    pub fn print_report(
        &self,
        clock: &Clock,
        memory_name: &str,
        activity_by_node_idx: &HashMap<usize, &PeActivity>,
    ) {
        let oversubscribed_fraction = (self.requested_fraction - 1.0).max(0.0);
        let active_nodes = self
            .active_node_indices
            .iter()
            .filter_map(|node_idx| {
                let activity = activity_by_node_idx.get(node_idx)?;
                activity.memory_summary_string(memory_name)
            })
            .collect::<Vec<_>>()
            .join(", ");

        println!(
            "    {:.2}ns -> {:.2}ns | requested={:.0}% oversubscribed={:.0}% active=[{}]",
            ticks_to_ns(clock, self.start_ticks),
            ticks_to_ns(clock, self.end_ticks),
            self.requested_fraction * 100.0,
            oversubscribed_fraction * 100.0,
            active_nodes
        );
    }
}

#[derive(Clone, Debug, Default)]
pub struct MemoryContentionAnalysis {
    pub windows: Vec<MemoryContentionWindow>,
    pub adjusted_ticks_by_node_idx: HashMap<usize, f64>,
}

impl MemoryContentionAnalysis {
    #[must_use]
    pub fn total_active_ticks(&self) -> f64 {
        self.windows
            .iter()
            .map(|window| window.end_ticks - window.start_ticks)
            .sum::<f64>()
    }

    #[must_use]
    pub fn achieved_bytes_per_tick(&self, memory_bandwidth: f64) -> f64 {
        let total_active_ticks = self.total_active_ticks();
        if total_active_ticks > 0.0 {
            self.windows
                .iter()
                .map(|window| {
                    let duration_ticks = window.end_ticks - window.start_ticks;
                    memory_bandwidth * window.requested_fraction.min(1.0) * duration_ticks
                })
                .sum::<f64>()
                / total_active_ticks
        } else {
            0.0
        }
    }

    #[must_use]
    pub fn average_oversubscription(&self) -> f64 {
        let total_active_ticks = self.total_active_ticks();
        if total_active_ticks > 0.0 {
            self.windows
                .iter()
                .map(|window| {
                    let duration_ticks = window.end_ticks - window.start_ticks;
                    let oversubscribed_fraction = (window.requested_fraction - 1.0).max(0.0);
                    oversubscribed_fraction * duration_ticks
                })
                .sum::<f64>()
                / total_active_ticks
        } else {
            0.0
        }
    }

    pub fn print_report(
        &self,
        clock: &Clock,
        platform: &Platform,
        memory_name: &str,
        activities: &[PeActivity],
        activity_by_node_idx: &HashMap<usize, &PeActivity>,
    ) {
        println!("  {memory_name}:");
        if self.windows.is_empty() {
            println!("    no scheduled activity");
            return;
        }

        let memory_bandwidth = platform.memory(memory_name).unwrap().bw_bytes_per_cycle() as f64;
        let total_bytes = activities
            .iter()
            .map(|activity| activity.node.memory_bytes(memory_name))
            .sum::<usize>();
        let achieved_bytes_per_tick = self.achieved_bytes_per_tick(memory_bandwidth);
        let average_oversubscription = self.average_oversubscription();

        println!(
            "    summary: bytes={total_bytes} ({:.2}) achieved_bw={achieved_bytes_per_tick:.2}/{memory_bandwidth:.2} bytes/tick ({:.2} GB/s) avg_oversubscription={:.1}%",
            format_bytes(total_bytes),
            bytes_per_tick_to_gb_per_s(clock, achieved_bytes_per_tick),
            average_oversubscription * 100.0
        );

        for window in &self.windows {
            window.print_report(clock, memory_name, activity_by_node_idx);
        }
    }
}

#[derive(Default)]
pub struct WidestPathCache {
    bandwidths: HashMap<(String, String), Option<usize>>,
}

pub struct BandwidthGraph {
    edges: HashMap<String, Vec<(String, usize)>>,
}

pub fn resource_bytes_per_cycle(
    platform: &Platform,
) -> Result<HashMap<String, usize>, Box<dyn std::error::Error>> {
    let mut capacities = HashMap::new();

    for name in platform.pe_names() {
        capacities.insert(
            format!("pe:{name}"),
            platform.pe(&name)?.lsu_access_bytes_per_tick(),
        );
    }

    for name in platform.cache_names() {
        capacities.insert(
            format!("cache:{name}"),
            platform.cache(&name)?.bw_bytes_per_cycle(),
        );
    }

    for name in platform.fabric_names() {
        capacities.insert(
            format!("fabric:{name}"),
            platform.fabric(&name)?.port_bits_per_tick().div_ceil(8),
        );
    }

    for name in platform.memory_names() {
        capacities.insert(
            format!("mem:{name}"),
            platform.memory(&name)?.bw_bytes_per_cycle(),
        );
    }

    Ok(capacities)
}

impl BandwidthGraph {
    fn canonical_resource_name(endpoint: &str) -> Result<String, Box<dyn std::error::Error>> {
        if let Some(name) = endpoint.strip_prefix("pe.") {
            return Ok(format!("pe:{name}"));
        }
        if let Some(rest) = endpoint.strip_prefix("cache.") {
            let name = rest
                .split('.')
                .next()
                .ok_or_else(|| format!("Failed to parse cache endpoint '{endpoint}'"))?;
            return Ok(format!("cache:{name}"));
        }
        if let Some(rest) = endpoint.strip_prefix("mem.") {
            let name = rest
                .split('.')
                .next()
                .ok_or_else(|| format!("Failed to parse memory endpoint '{endpoint}'"))?;
            return Ok(format!("mem:{name}"));
        }
        if let Some(rest) = endpoint.strip_prefix("fabric.") {
            let name = rest
                .split('@')
                .next()
                .ok_or_else(|| format!("Failed to parse fabric endpoint '{endpoint}'"))?;
            return Ok(format!("fabric:{name}"));
        }
        Err(format!("Unsupported connection endpoint '{endpoint}'").into())
    }

    pub fn build(
        cfg: &gwr_platform::types::PlatformConfig,
        bytes_per_cycle: &HashMap<String, usize>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut edges: HashMap<String, Vec<(String, usize)>> = HashMap::new();

        if let Some(connections) = &cfg.connections {
            for connection in connections {
                if connection.connect.len() != 2 {
                    return Err(format!(
                        "Invalid connection with {} endpoints",
                        connection.connect.len()
                    )
                    .into());
                }

                let from = Self::canonical_resource_name(&connection.connect[0])?;
                let to = Self::canonical_resource_name(&connection.connect[1])?;
                let from_capacity = bytes_per_cycle
                    .get(&from)
                    .ok_or_else(|| format!("Missing bandwidth capacity for '{from}'"))?;
                let to_capacity = bytes_per_cycle
                    .get(&to)
                    .ok_or_else(|| format!("Missing bandwidth capacity for '{to}'"))?;
                let edge_capacity = (*from_capacity).min(*to_capacity);

                edges
                    .entry(from.clone())
                    .or_default()
                    .push((to.clone(), edge_capacity));
                edges.entry(to).or_default().push((from, edge_capacity));
            }
        }

        Ok(Self { edges })
    }

    fn widest_path_cache_key(from: &str, to: &str) -> (String, String) {
        if from <= to {
            (from.to_string(), to.to_string())
        } else {
            (to.to_string(), from.to_string())
        }
    }

    fn widest_path_bandwidth(&self, from: &str, to: &str) -> Option<usize> {
        let mut best: HashMap<&str, usize> = HashMap::new();
        let mut heap = BinaryHeap::new();

        best.insert(from, usize::MAX);
        heap.push((usize::MAX, from));

        while let Some((width, node)) = heap.pop() {
            if node == to {
                log::debug!("widest_path_bandwidth: '{from}' -> '{to}' with width={width} bytes");
                return Some(width);
            }

            if best.get(node).copied().unwrap_or(0) > width {
                continue;
            }

            for (next, edge_capacity) in self.edges.get(node)? {
                let next_width = width.min(*edge_capacity);
                let prev = best.get(next.as_str()).copied().unwrap_or(0);
                if next_width > prev {
                    best.insert(next, next_width);
                    heap.push((next_width, next.as_str()));
                }
            }
        }

        None
    }

    pub fn cached_widest_path_bandwidth(
        &self,
        cache: &mut WidestPathCache,
        from: &str,
        to: &str,
    ) -> Option<usize> {
        let key = Self::widest_path_cache_key(from, to);
        if let Some(width) = cache.bandwidths.get(&key) {
            log::debug!("widest_path_bandwidth cache hit: '{from}' <-> '{to}' -> {width:?}");
            return *width;
        }

        log::debug!("widest_path_bandwidth cache miss: '{from}' <-> '{to}'");
        let width = self.widest_path_bandwidth(from, to);
        cache.bandwidths.insert(key, width);
        width
    }
}
