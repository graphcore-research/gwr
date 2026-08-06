// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::BuildHasher;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::SimError;
use gwr_models::fabric::functional::FunctionalFabric;
use gwr_models::fabric::node::FabricRoutingAlgorithm;
use gwr_models::fabric::routed::RoutedFabric;
use gwr_models::fabric::{Fabric, FabricConfig, FabricPortSelection};
use gwr_models::memory::cache::{Cache, CacheConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::processing_element::{ProcessingElement, ProcessingElementConfig};
use gwr_track::entity::{Entity, GetEntity};

use crate::connection_id::{CachePortId, ConnectionEndpointId, parse_connection_endpoint_id};
use crate::types::{
    FabricKind, FabricSection, MemoryMapSection, PlatformConfig, ProcessingElementConfigSection,
};
use crate::{Caches, DeviceIds, Fabrics, Memories, NameToIdxMap, ProcessingElements};

pub const DEFAULT_HBM_DELAY_TICKS: usize = 10;
pub const DEFAULT_HBM_BW_BYTES_PER_CYCLE: usize = 32;
pub const DEFAULT_HBM_SIZE_BYTES: usize = 1024 * 1024 * 1024;

pub fn build_memories(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
) -> Result<(Memories, NameToIdxMap), SimError> {
    let mut memories = Vec::new();
    if let Some(memories_section) = &cfg.memories {
        for memory_section in memories_section {
            let base_address = memory_section.base_address;
            let capacity_bytes = memory_section.capacity_bytes as usize;
            let bw_bytes_per_cycle = memory_section
                .bw_bytes_per_cycle
                .unwrap_or(DEFAULT_HBM_BW_BYTES_PER_CYCLE);
            let delay_ticks = memory_section
                .delay_ticks
                .unwrap_or(DEFAULT_HBM_DELAY_TICKS);
            let config = MemoryConfig::new(
                base_address,
                capacity_bytes,
                bw_bytes_per_cycle,
                delay_ticks,
            );
            memories.push(Memory::new_and_register(
                engine,
                clock,
                parent,
                memory_section.name.as_str(),
                config,
            )?);
        }
    }

    let mut memories_idx_by_id = HashMap::new();
    for (i, memory) in memories.iter().enumerate() {
        let name = memory.entity().name.to_string();
        memories_idx_by_id.insert(name, i);
    }

    Ok((memories, memories_idx_by_id))
}

pub fn build_memory_maps(
    cfg: &PlatformConfig,
    memories: &Memories,
    memories_idx_by_id: &NameToIdxMap,
    device_ids: &DeviceIds,
) -> Result<HashMap<String, Rc<MemoryMap>>, SimError> {
    let mut memory_maps = HashMap::new();
    for memory_map in &cfg.memory_maps {
        let built = build_memory_map(memory_map, memories, memories_idx_by_id, device_ids)?;
        memory_maps.insert(memory_map.name.clone(), Rc::new(built));
    }

    Ok(memory_maps)
}

pub fn build_memory_map(
    cfg: &MemoryMapSection,
    memories: &Memories,
    memories_idx_by_id: &NameToIdxMap,
    device_ids: &DeviceIds,
) -> Result<MemoryMap, SimError> {
    let mut memory_map = MemoryMap::new();
    for device in &cfg.devices {
        let memory_idx = memories_idx_by_id
            .get(device.name.as_str())
            .ok_or_else(|| SimError(format!("Unknown memory '{}'", device.name)))?;
        let memory = &memories[*memory_idx];
        let device_id = *device_ids
            .get(&device.name)
            .ok_or_else(|| SimError(format!("Unknown device '{}'", device.name)))?;
        memory_map.insert(
            memory.base_address(),
            memory.capacity_bytes() as u64,
            device_id,
        )?;
    }
    Ok(memory_map)
}

pub const DEFAULT_PE_NUM_ACTIVE_REQUESTS: usize = 8;
pub const DEFAULT_PE_LSU_ACCESS_BYTES: usize = 32;
pub const DEFAULT_PE_SRAM_BYTES: u64 = 1024 * 1024;
pub const DEFAULT_PE_ADDS_PER_TICK: f64 = 16.0;
pub const DEFAULT_PE_MULS_PER_TICK: f64 = 4.0;
pub const DEFAULT_PE_COMPARES_PER_TICK: f64 = DEFAULT_PE_ADDS_PER_TICK;
pub const DEFAULT_PE_OVERHEAD_SIZE_BYTES: usize = 8;

pub fn build_pes<S: BuildHasher>(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
    memory_maps: &HashMap<String, Rc<MemoryMap>, S>,
    device_ids: &DeviceIds,
) -> Result<(ProcessingElements, NameToIdxMap), SimError> {
    let mut processing_elements = Vec::new();
    if let Some(pes) = &cfg.processing_elements {
        for pe_section in pes {
            let memory_map = memory_maps
                .get(pe_section.memory_map.as_str())
                .ok_or_else(|| {
                    SimError(format!("Unknown memory map '{}'", pe_section.memory_map))
                })?;
            let device_id = *device_ids
                .get(&pe_section.name)
                .ok_or_else(|| SimError(format!("Unknown device '{}'", pe_section.name)))?;
            let pe_config = build_pe_config(&pe_section.config)?;
            processing_elements.push(ProcessingElement::new_and_register(
                engine,
                clock,
                parent,
                pe_section.name.as_str(),
                memory_map,
                &pe_config,
                device_id,
            )?);
        }
    }
    let mut pes_idx_by_id = HashMap::new();
    for (i, pe) in processing_elements.iter().enumerate() {
        let name = pe.entity().name.to_string();
        pes_idx_by_id.insert(name, i);
    }
    Ok((processing_elements, pes_idx_by_id))
}

fn build_pe_config(
    cfg: &ProcessingElementConfigSection,
) -> Result<ProcessingElementConfig, SimError> {
    let num_active_requests = cfg
        .num_active_requests
        .unwrap_or(DEFAULT_PE_NUM_ACTIVE_REQUESTS);
    let lsu_access_bytes = cfg.lsu_access_bytes.unwrap_or(DEFAULT_PE_LSU_ACCESS_BYTES);
    let overhead_size_bytes = cfg
        .overhead_size_bytes
        .unwrap_or(DEFAULT_PE_OVERHEAD_SIZE_BYTES);
    let sram_bytes = cfg.sram_bytes.unwrap_or(DEFAULT_PE_SRAM_BYTES) as usize;

    let adds_per_tick = cfg.adds_per_tick.unwrap_or(DEFAULT_PE_ADDS_PER_TICK);
    let muls_per_tick = cfg.muls_per_tick.unwrap_or(DEFAULT_PE_MULS_PER_TICK);
    let compares_per_tick = cfg
        .compares_per_tick
        .unwrap_or(DEFAULT_PE_COMPARES_PER_TICK);

    Ok(ProcessingElementConfig {
        num_active_requests,
        lsu_access_bytes,
        overhead_size_bytes,
        sram_bytes,
        adds_per_tick,
        muls_per_tick,
        compares_per_tick,
    })
}

pub const DEFAULT_CACHE_LINE_SIZE_BYTES: usize = 32;
pub const DEFAULT_CACHE_BW_BYTES_PER_CYCLE: usize = 32;
pub const DEFAULT_CACHE_NUM_WAYS: usize = 4;
pub const DEFAULT_CACHE_NUM_SETS: usize = 128;
pub const DEFAULT_CACHE_LATENCY_TICKS: usize = 20;

pub fn build_caches(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
) -> Result<(Caches, NameToIdxMap), SimError> {
    let mut caches = Vec::new();
    if let Some(caches_sections) = &cfg.caches {
        for cache_section in caches_sections {
            let bw_bytes_per_cycle = cache_section
                .config
                .bw_bytes_per_cycle
                .unwrap_or(DEFAULT_CACHE_BW_BYTES_PER_CYCLE);
            let line_size_bytes = cache_section
                .config
                .line_size_bytes
                .unwrap_or(DEFAULT_CACHE_LINE_SIZE_BYTES);
            let num_sets = cache_section
                .config
                .num_sets
                .unwrap_or(DEFAULT_CACHE_NUM_SETS);
            let num_ways = cache_section
                .config
                .num_ways
                .unwrap_or(DEFAULT_CACHE_NUM_WAYS);
            let delay_ticks = cache_section
                .config
                .delay_ticks
                .unwrap_or(DEFAULT_CACHE_LATENCY_TICKS);

            let config = CacheConfig::new(
                line_size_bytes,
                bw_bytes_per_cycle,
                num_sets,
                num_ways,
                delay_ticks,
            );
            caches.push(Cache::new_and_register(
                engine,
                clock,
                parent,
                cache_section.name.as_str(),
                config,
            )?);
        }
    }

    let mut caches_idx_by_id = HashMap::new();
    for (i, pe) in caches.iter().enumerate() {
        let name = pe.entity().name.to_string();
        caches_idx_by_id.insert(name, i);
    }

    Ok((caches, caches_idx_by_id))
}

pub const DEFAULT_FABRIC_PORTS_PER_NODE: usize = 1;
pub const DEFAULT_FABRIC_TICKS_PER_HOP: usize = 2;
pub const DEFAULT_FABRIC_TICKS_OVERHEAD: usize = 10;
pub const DEFAULT_FABRIC_RX_BUFFER_BYTES: usize = 256;
pub const DEFAULT_FABRIC_TX_BUFFER_BYTES: usize = 256;
pub const DEFAULT_FABRIC_PORT_BITS_PER_TICK: usize = 32 * 8; // 32 bytes per cycle
pub const DEFAULT_FABRIC_ROUTING: FabricRoutingAlgorithm = FabricRoutingAlgorithm::ColumnFirst;
pub const DEFAULT_FABRIC_PORT_SELECTION: FabricPortSelection =
    FabricPortSelection::DestinationAddressHash;

pub type FabricDestinationPortMap = HashMap<u64, Vec<usize>>;
pub type FabricDestinationPortMaps = HashMap<String, FabricDestinationPortMap>;

pub fn build_fabric_destination_port_maps(
    cfg: &PlatformConfig,
    device_ids: &DeviceIds,
) -> Result<FabricDestinationPortMaps, SimError> {
    let topology = FabricTopology::new(cfg)?;
    let mut maps = FabricDestinationPortMaps::new();

    if let Some(fabrics) = &cfg.fabrics {
        for fabric in fabrics {
            let mut destination_port_map: FabricDestinationPortMap = HashMap::new();
            let Some(config) = topology.fabric_configs.get(&fabric.name) else {
                continue;
            };

            let mut shortest_distance_by_device = HashMap::new();
            for port_idx in config.port_indices() {
                for (device_id, distance) in
                    topology.device_distances_from_port(&fabric.name, *port_idx, device_ids)
                {
                    let shortest = shortest_distance_by_device
                        .entry(device_id)
                        .or_insert(distance);
                    if distance < *shortest {
                        *shortest = distance;
                        destination_port_map.insert(device_id, vec![*port_idx]);
                    } else if distance == *shortest {
                        destination_port_map
                            .entry(device_id)
                            .or_default()
                            .push(*port_idx);
                    }
                }
            }

            for ports in destination_port_map.values_mut() {
                ports.sort_unstable();
                ports.dedup();
            }
            maps.insert(fabric.name.clone(), destination_port_map);
        }
    }

    Ok(maps)
}

pub fn build_fabrics(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
    fabric_destination_port_maps: &FabricDestinationPortMaps,
) -> Result<(Fabrics, NameToIdxMap), SimError> {
    let mut fabrics = Vec::new();
    if let Some(fabric_sections) = &cfg.fabrics {
        for fabric_section in fabric_sections {
            let fabric_algorithm = fabric_section.routing.unwrap_or(DEFAULT_FABRIC_ROUTING);
            let destination_port_map = fabric_destination_port_maps
                .get(&fabric_section.name)
                .cloned()
                .unwrap_or_default();
            let config = Rc::new(build_fabric_config(fabric_section, destination_port_map)?);

            let fabric: Rc<dyn Fabric<MemoryAccess>> = match fabric_section.kind {
                FabricKind::Functional => FunctionalFabric::new_and_register(
                    engine,
                    clock,
                    parent,
                    &fabric_section.name,
                    config.clone(),
                )?,
                FabricKind::Routed => RoutedFabric::new_and_register(
                    engine,
                    clock,
                    parent,
                    &fabric_section.name,
                    config.clone(),
                    fabric_algorithm,
                )?,
            };
            fabrics.push(fabric);
        }
    }

    let mut fabrics_idx_by_id = HashMap::new();
    for (i, fabric) in fabrics.iter().enumerate() {
        let name = fabric.entity().name.to_string();
        fabrics_idx_by_id.insert(name, i);
    }

    Ok((fabrics, fabrics_idx_by_id))
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum ResolvedEndpoint {
    Pe(String),
    CacheDev(String),
    CacheMem(String),
    Mem(String),
    FabricPort { fabric: String, port_idx: usize },
}

struct FabricTopology {
    edges: HashMap<ResolvedEndpoint, HashSet<ResolvedEndpoint>>,
    fabric_configs: HashMap<String, FabricConfig>,
}

impl FabricTopology {
    fn new(cfg: &PlatformConfig) -> Result<Self, SimError> {
        let mut topology = Self {
            edges: HashMap::new(),
            fabric_configs: build_empty_fabric_configs(cfg)?,
        };
        topology.add_cache_passthroughs(cfg);
        topology.add_connection_edges(cfg)?;
        Ok(topology)
    }

    fn add_edge(&mut self, a: ResolvedEndpoint, b: ResolvedEndpoint) {
        self.edges.entry(a.clone()).or_default().insert(b.clone());
        self.edges.entry(b).or_default().insert(a);
    }

    fn add_cache_passthroughs(&mut self, cfg: &PlatformConfig) {
        if let Some(caches) = &cfg.caches {
            for cache in caches {
                self.add_edge(
                    ResolvedEndpoint::CacheDev(cache.name.clone()),
                    ResolvedEndpoint::CacheMem(cache.name.clone()),
                );
            }
        }
    }

    fn add_connection_edges(&mut self, cfg: &PlatformConfig) -> Result<(), SimError> {
        if let Some(connections) = &cfg.connections {
            for c in connections {
                if c.connect.len() != 2 {
                    return Err(SimError(format!(
                        "Invalid 'connect' with {} entries (only 2 expected)",
                        c.connect.len()
                    )));
                }
                let (a, b) = self.parse_topology_connection(&c.connect[0], &c.connect[1])?;
                self.add_edge(a, b);
            }
        }
        Ok(())
    }

    fn device_distances_from_port(
        &self,
        target_fabric: &str,
        start_port_idx: usize,
        device_ids: &DeviceIds,
    ) -> HashMap<u64, usize> {
        let mut found = HashMap::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let start = ResolvedEndpoint::FabricPort {
            fabric: target_fabric.to_string(),
            port_idx: start_port_idx,
        };
        visited.insert(start.clone());
        queue.push_back((start, 0));

        while let Some((node, distance)) = queue.pop_front() {
            match &node {
                ResolvedEndpoint::Pe(name) | ResolvedEndpoint::Mem(name) => {
                    if let Some(device_id) = device_ids.get(name) {
                        found.entry(device_id.0).or_insert(distance);
                    }
                }
                _ => {}
            }

            if let Some(neighbors) = self.edges.get(&node) {
                for neighbor in neighbors {
                    if visited.insert(neighbor.clone()) {
                        queue.push_back((neighbor.clone(), distance + 1));
                    }
                }
            }

            if let ResolvedEndpoint::FabricPort { fabric, .. } = &node
                && fabric != target_fabric
                && let Some(config) = self.fabric_configs.get(fabric)
            {
                for port_idx in config.port_indices() {
                    let neighbor = ResolvedEndpoint::FabricPort {
                        fabric: fabric.clone(),
                        port_idx: *port_idx,
                    };
                    if visited.insert(neighbor.clone()) {
                        queue.push_back((neighbor, distance + 1));
                    }
                }
            }
        }

        found
    }

    fn parse_topology_connection(
        &self,
        from: &str,
        to: &str,
    ) -> Result<(ResolvedEndpoint, ResolvedEndpoint), SimError> {
        let from_raw = parse_connection_endpoint_id(from)?;
        let to_raw = parse_connection_endpoint_id(to)?;
        Ok((
            self.resolve_topology_node(&from_raw, &to_raw, true)?,
            self.resolve_topology_node(&to_raw, &from_raw, false)?,
        ))
    }

    fn resolve_topology_node(
        &self,
        node: &ConnectionEndpointId,
        other: &ConnectionEndpointId,
        is_from: bool,
    ) -> Result<ResolvedEndpoint, SimError> {
        match node {
            ConnectionEndpointId::Pe { name } => Ok(ResolvedEndpoint::Pe(name.clone())),
            ConnectionEndpointId::Mem { name } => Ok(ResolvedEndpoint::Mem(name.clone())),
            ConnectionEndpointId::FabricPort {
                fabric,
                column,
                row,
                port,
            } => Ok(ResolvedEndpoint::FabricPort {
                fabric: fabric.clone(),
                port_idx: self.fabric_port_endpoint_to_index(fabric, *column, *row, *port)?,
            }),
            ConnectionEndpointId::Cache { name, port } => match port {
                Some(CachePortId::Dev) => Ok(ResolvedEndpoint::CacheDev(name.clone())),
                Some(CachePortId::Mem) => Ok(ResolvedEndpoint::CacheMem(name.clone())),
                None => match other {
                    ConnectionEndpointId::Pe { .. } => Ok(ResolvedEndpoint::CacheDev(name.clone())),
                    ConnectionEndpointId::Cache { .. } if is_from => {
                        Ok(ResolvedEndpoint::CacheMem(name.clone()))
                    }
                    ConnectionEndpointId::Cache { .. } => {
                        Ok(ResolvedEndpoint::CacheDev(name.clone()))
                    }
                    ConnectionEndpointId::Mem { .. } | ConnectionEndpointId::FabricPort { .. } => {
                        Ok(ResolvedEndpoint::CacheMem(name.clone()))
                    }
                },
            },
        }
    }

    fn fabric_port_endpoint_to_index(
        &self,
        fabric: &str,
        column: usize,
        row: usize,
        port: usize,
    ) -> Result<usize, SimError> {
        let config = self
            .fabric_configs
            .get(fabric)
            .ok_or_else(|| SimError(format!("Unknown fabric '{fabric}'")))?;
        if column >= config.num_columns() {
            return Err(SimError(format!(
                "Fabric '{fabric}' column {column} is out of range"
            )));
        }
        if row >= config.num_rows() {
            return Err(SimError(format!(
                "Fabric '{fabric}' row {row} is out of range"
            )));
        }

        if port >= config.node_num_ingress_egress_ports(column, row) {
            return Err(SimError(format!(
                "Fabric '{fabric}' port ({column},{row},{port}) is not populated"
            )));
        }
        Ok(config.col_row_port_to_fabric_port_index(column, row, port))
    }
}

fn build_empty_fabric_configs(
    cfg: &PlatformConfig,
) -> Result<HashMap<String, FabricConfig>, SimError> {
    // These configs are topology-only: the destination port maps have not been
    // derived yet, so they are built empty and used only for fabric geometry.
    let mut fabric_configs = HashMap::new();
    if let Some(fabrics) = &cfg.fabrics {
        for fabric in fabrics {
            let config = build_fabric_config(fabric, HashMap::new())?;
            fabric_configs.insert(fabric.name.clone(), config);
        }
    }
    Ok(fabric_configs)
}

fn build_fabric_config(
    fabric_section: &FabricSection,
    destination_port_map: FabricDestinationPortMap,
) -> Result<FabricConfig, SimError> {
    let fabric_columns = fabric_section.columns;
    let fabric_rows = fabric_section.rows;
    let fabric_ports_per_node = fabric_section
        .fabric_ports_per_node
        .unwrap_or(DEFAULT_FABRIC_PORTS_PER_NODE);
    let ticks_per_hop = fabric_section
        .ticks_per_hop
        .unwrap_or(DEFAULT_FABRIC_TICKS_PER_HOP);
    let ticks_overhead = fabric_section
        .ticks_overhead
        .unwrap_or(DEFAULT_FABRIC_TICKS_OVERHEAD);
    let rx_buffer_bytes = fabric_section
        .rx_buffer_bytes
        .unwrap_or(DEFAULT_FABRIC_RX_BUFFER_BYTES);
    let tx_buffer_bytes = fabric_section
        .tx_buffer_bytes
        .unwrap_or(DEFAULT_FABRIC_TX_BUFFER_BYTES);
    let port_bits_per_tick = fabric_section
        .port_bits_per_tick
        .unwrap_or(DEFAULT_FABRIC_PORT_BITS_PER_TICK);
    let port_selection = fabric_section
        .port_selection
        .unwrap_or(DEFAULT_FABRIC_PORT_SELECTION);

    Ok(FabricConfig::new(
        fabric_columns,
        fabric_rows,
        fabric_ports_per_node,
        None,
        ticks_per_hop,
        ticks_overhead,
        rx_buffer_bytes,
        tx_buffer_bytes,
        port_bits_per_tick,
        destination_port_map,
    )?
    .with_port_selection(port_selection))
}

#[cfg(test)]
mod tests {
    use gwr_engine::test_helpers::start_test;
    use gwr_engine::types::DeviceId;

    use super::{build_fabric_destination_port_maps, build_memories, build_memory_maps};
    use crate::DeviceIds;
    use crate::types::{
        MemoryDeviceSection, MemoryKind, MemoryMapSection, MemorySection, PlatformConfig,
    };

    #[test]
    fn builds_runtime_memory_maps_from_built_memories() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let cfg = PlatformConfig {
            memory_maps: vec![MemoryMapSection {
                name: "mm0".to_string(),
                devices: vec![MemoryDeviceSection {
                    name: "hbm0".to_string(),
                }],
            }],
            defaults: None,
            processing_elements: None,
            caches: None,
            fabrics: None,
            memories: Some(vec![MemorySection {
                name: "hbm0".to_string(),
                kind: MemoryKind::HBM,
                base_address: 0x4000,
                capacity_bytes: 0x2000,
                bw_bytes_per_cycle: None,
                delay_ticks: None,
            }]),
            connections: None,
        };
        let device_ids = DeviceIds::from([("hbm0".to_string(), DeviceId(7))]);
        let (memories, memories_idx_by_id) = build_memories(&engine, &clock, engine.top(), &cfg)
            .expect("memory build should succeed");

        let memory_maps = build_memory_maps(&cfg, &memories, &memories_idx_by_id, &device_ids)
            .expect("memory maps should build");
        let memory_map = memory_maps.get("mm0").expect("memory map should exist");

        assert_eq!(memory_map.num_regions(), 1);
        assert_eq!(memory_map.lookup(0x4000), Some((DeviceId(7), 0)));
        assert_eq!(memory_map.lookup(0x5fff), Some((DeviceId(7), 0x1fff)));
        assert_eq!(memory_map.lookup(0x6000), None);
    }

    #[test]
    fn derives_fabric_port_maps_from_direct_connections() {
        let cfg: PlatformConfig = serde_yaml::from_str(
            "
memory_maps:
  - name: mm0
    devices:
      - name: hbm0
processing_elements:
  - name: pe0
    memory_map: mm0
    config: {}
fabrics:
  - name: fabric0
    kind: functional
    columns: 2
    rows: 1
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 1024
connections:
  - connect:
    - pe.pe0
    - fabric.fabric0@(0,0)
  - connect:
    - mem.hbm0
    - fabric.fabric0@(1,0)
",
        )
        .expect("platform yaml should parse");
        let device_ids = DeviceIds::from([
            ("pe0".to_string(), DeviceId(0)),
            ("hbm0".to_string(), DeviceId(1)),
        ]);

        let maps = build_fabric_destination_port_maps(&cfg, &device_ids)
            .expect("fabric maps should build");
        let fabric_map = maps.get("fabric0").expect("fabric map should exist");

        assert_eq!(fabric_map.get(&0), Some(&vec![0]));
        assert_eq!(fabric_map.get(&1), Some(&vec![1]));
    }

    #[test]
    fn derives_fabric_port_maps_through_cache() {
        let cfg: PlatformConfig = serde_yaml::from_str(
            "
memory_maps:
  - name: mm0
    devices:
      - name: hbm0
processing_elements:
  - name: pe0
    memory_map: mm0
    config: {}
caches:
  - name: l1
    config: {}
fabrics:
  - name: fabric0
    kind: functional
    columns: 2
    rows: 1
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 1024
connections:
  - connect:
    - pe.pe0
    - cache.l1
  - connect:
    - cache.l1
    - fabric.fabric0@(0,0)
  - connect:
    - mem.hbm0
    - fabric.fabric0@(1,0)
",
        )
        .expect("platform yaml should parse");
        let device_ids = DeviceIds::from([
            ("pe0".to_string(), DeviceId(0)),
            ("hbm0".to_string(), DeviceId(1)),
        ]);

        let maps = build_fabric_destination_port_maps(&cfg, &device_ids)
            .expect("fabric maps should build");
        let fabric_map = maps.get("fabric0").expect("fabric map should exist");

        assert_eq!(fabric_map.get(&0), Some(&vec![0]));
        assert_eq!(fabric_map.get(&1), Some(&vec![1]));
    }

    #[test]
    fn derives_multiple_candidate_ports_for_same_device() {
        let cfg: PlatformConfig = serde_yaml::from_str(
            "
memory_maps:
  - name: mm0
    devices:
      - name: hbm0
fabrics:
  - name: fabric0
    kind: functional
    columns: 2
    rows: 1
    fabric_ports_per_node: 2
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 1024
connections:
  - connect:
    - mem.hbm0
    - fabric.fabric0@(1,0)
  - connect:
    - mem.hbm0
    - fabric.fabric0@(1,0).1
",
        )
        .expect("platform yaml should parse");
        let device_ids = DeviceIds::from([("hbm0".to_string(), DeviceId(7))]);

        let maps = build_fabric_destination_port_maps(&cfg, &device_ids)
            .expect("fabric maps should build");
        let fabric_map = maps.get("fabric0").expect("fabric map should exist");

        assert_eq!(fabric_map.get(&7), Some(&vec![2, 3]));
    }

    #[test]
    fn derives_fabric_port_maps_through_single_fabric_link() {
        let cfg: PlatformConfig = serde_yaml::from_str(
            "
memory_maps:
  - name: mm0
    devices: []
processing_elements:
  - name: pe0
    memory_map: mm0
    config: {}
  - name: pe1
    memory_map: mm0
    config: {}
fabrics:
  - name: fabric0
    kind: functional
    columns: 2
    rows: 1
  - name: fabric1
    kind: functional
    columns: 2
    rows: 1
connections:
  - connect:
    - pe.pe0
    - fabric.fabric0@(0,0)
  - connect:
    - pe.pe1
    - fabric.fabric1@(0,0)
  - connect:
    - fabric.fabric0@(1,0)
    - fabric.fabric1@(1,0)
",
        )
        .expect("platform yaml should parse");
        let device_ids = DeviceIds::from([
            ("pe0".to_string(), DeviceId(0)),
            ("pe1".to_string(), DeviceId(1)),
        ]);

        let maps = build_fabric_destination_port_maps(&cfg, &device_ids)
            .expect("fabric maps should build");
        let fabric0_map = maps.get("fabric0").expect("fabric0 map should exist");
        let fabric1_map = maps.get("fabric1").expect("fabric1 map should exist");

        assert_eq!(fabric0_map.get(&0), Some(&vec![0]));
        assert_eq!(fabric0_map.get(&1), Some(&vec![1]));
        assert_eq!(fabric1_map.get(&0), Some(&vec![1]));
        assert_eq!(fabric1_map.get(&1), Some(&vec![0]));
    }

    #[test]
    fn derives_fabric_port_maps_through_two_fabric_links() {
        let cfg: PlatformConfig = serde_yaml::from_str(
            "
memory_maps:
  - name: mm0
    devices: []
processing_elements:
  - name: pe0
    memory_map: mm0
    config: {}
  - name: pe1
    memory_map: mm0
    config: {}
fabrics:
  - name: fabric0
    kind: functional
    columns: 3
    rows: 1
  - name: fabric1
    kind: functional
    columns: 3
    rows: 1
connections:
  - connect:
    - pe.pe0
    - fabric.fabric0@(0,0)
  - connect:
    - fabric.fabric0@(1,0)
    - fabric.fabric1@(1,0)
  - connect:
    - fabric.fabric0@(2,0)
    - fabric.fabric1@(0,0)
  - connect:
    - pe.pe1
    - fabric.fabric1@(2,0)
",
        )
        .expect("platform yaml should parse");
        let device_ids = DeviceIds::from([
            ("pe0".to_string(), DeviceId(0)),
            ("pe1".to_string(), DeviceId(1)),
        ]);

        let maps = build_fabric_destination_port_maps(&cfg, &device_ids)
            .expect("fabric maps should build");
        let fabric0_map = maps.get("fabric0").expect("fabric0 map should exist");
        let fabric1_map = maps.get("fabric1").expect("fabric1 map should exist");

        assert_eq!(fabric0_map.get(&0), Some(&vec![0]));
        assert_eq!(fabric0_map.get(&1), Some(&vec![1, 2]));
        assert_eq!(fabric1_map.get(&0), Some(&vec![0, 1]));
        assert_eq!(fabric1_map.get(&1), Some(&vec![2]));
    }

    #[test]
    fn derives_only_shortest_fabric_port_maps_through_cycle() {
        let cfg: PlatformConfig = serde_yaml::from_str(
            "
memory_maps:
  - name: mm0
    devices: []
processing_elements:
  - name: pe1
    memory_map: mm0
    config: {}
fabrics:
  - name: fabric0
    kind: functional
    columns: 3
    rows: 1
  - name: fabric1
    kind: functional
    columns: 3
    rows: 1
  - name: fabric2
    kind: functional
    columns: 3
    rows: 1
connections:
  - connect:
    - pe.pe1
    - fabric.fabric1@(0,0)
  - connect:
    - fabric.fabric0@(1,0)
    - fabric.fabric1@(1,0)
  - connect:
    - fabric.fabric1@(2,0)
    - fabric.fabric2@(1,0)
  - connect:
    - fabric.fabric2@(2,0)
    - fabric.fabric0@(2,0)
",
        )
        .expect("platform yaml should parse");
        let device_ids = DeviceIds::from([("pe1".to_string(), DeviceId(1))]);

        let maps = build_fabric_destination_port_maps(&cfg, &device_ids)
            .expect("fabric maps should build");
        let fabric0_map = maps.get("fabric0").expect("fabric0 map should exist");
        let fabric1_map = maps.get("fabric1").expect("fabric1 map should exist");
        let fabric2_map = maps.get("fabric2").expect("fabric2 map should exist");

        assert_eq!(fabric0_map.get(&1), Some(&vec![1]));
        assert_eq!(fabric1_map.get(&1), Some(&vec![0]));
        assert_eq!(fabric2_map.get(&1), Some(&vec![1]));
    }
}
