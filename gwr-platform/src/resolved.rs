// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use gwr_engine::types::{DeviceId, SimError};
use gwr_models::cache::coherency_manager::CoherencyManagerConfig;
use gwr_models::cache::{CacheConfig, CoherenceConfig};
use gwr_models::fabric::FabricConfig;
use gwr_models::fabric::node::FabricRoutingAlgorithm;
use gwr_models::memory::MemoryConfig;
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::processing_element::ProcessingElementConfig;

use crate::builder::DEFAULT_CACHE_LINE_SIZE_BYTES;
use crate::connection_id::{CachePortId, ConnectionEndpointId};
use crate::types::{
    CacheSection, FabricKind, FabricSection, MemorySection, PlatformConfig,
    ProcessingElementSection,
};
use crate::{DeviceIds, assign_device_ids};

// Pair each effective configuration with its source during validation. This
// transient borrow disappears before any runtime object is instantiated.
pub(crate) struct Configured<'a, S, C> {
    pub section: &'a S,
    pub config: C,
}

pub(crate) struct ModelConfigs<'a> {
    pub processing_elements: Vec<Configured<'a, ProcessingElementSection, ProcessingElementConfig>>,
    pub memories: Vec<Configured<'a, MemorySection, MemoryConfig>>,
    pub caches: Vec<Configured<'a, CacheSection, CacheConfig>>,
    pub fabrics: Vec<Configured<'a, FabricSection, (FabricConfig, FabricRoutingAlgorithm)>>,
}

impl<'a> ModelConfigs<'a> {
    pub fn new(config: &'a PlatformConfig) -> Result<Self, SimError> {
        Ok(Self {
            processing_elements: config
                .processing_elements
                .iter()
                .flatten()
                .map(|section| {
                    Ok(Configured {
                        section,
                        config: section.effective_config(&config.operation_classes)?,
                    })
                })
                .collect::<Result<_, SimError>>()?,
            memories: config
                .memories
                .iter()
                .flatten()
                .map(|section| {
                    Ok(Configured {
                        section,
                        config: section.effective_config()?,
                    })
                })
                .collect::<Result<_, SimError>>()?,
            caches: config
                .caches
                .iter()
                .flatten()
                .map(|section| {
                    Ok(Configured {
                        section,
                        config: section.effective_config()?,
                    })
                })
                .collect::<Result<_, SimError>>()?,
            fabrics: config
                .fabrics
                .iter()
                .flatten()
                .map(|section| {
                    Ok(Configured {
                        section,
                        config: section.effective_config(HashMap::new())?,
                    })
                })
                .collect::<Result<_, SimError>>()?,
        })
    }
}

pub(crate) struct ResolvedPe {
    pub name: String,
    pub config: ProcessingElementConfig,
    pub device_id: DeviceId,
    pub memory_map: Rc<MemoryMap>,
}

pub(crate) struct ResolvedMemory {
    pub name: String,
    pub config: MemoryConfig,
}

pub(crate) struct ResolvedCache {
    pub name: String,
    pub config: CacheConfig,
    pub coherence: Option<CoherenceConfig>,
}

pub(crate) struct ResolvedFabric {
    pub name: String,
    pub kind: FabricKind,
    pub config: Rc<FabricConfig>,
    pub routing: FabricRoutingAlgorithm,
}

pub(crate) struct ResolvedPlatform {
    pub pes: Vec<ResolvedPe>,
    pub memories: Vec<ResolvedMemory>,
    pub caches: Vec<ResolvedCache>,
    pub managers: Vec<(String, CoherencyManagerConfig)>,
    pub fabrics: Vec<ResolvedFabric>,
    pub connections: Vec<[Endpoint; 2]>,
    pub device_names_by_id: HashMap<u64, String>,
    pub reachable_memories: Vec<HashSet<DeviceId>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Endpoint {
    Null,
    Pe(usize),
    Memory(usize),
    Cache(usize, CachePortId),
    Manager(usize),
    Fabric(usize, usize),
}

impl ResolvedPlatform {
    pub fn new(
        config: &PlatformConfig,
        models: ModelConfigs<'_>,
        connections: Vec<[ConnectionEndpointId; 2]>,
    ) -> Result<Self, SimError> {
        let device_ids = assign_device_ids(config)?;
        let memory_maps = resolve_memory_maps(config, &models.memories, &device_ids)?;
        let managers = resolve_managers(config, &memory_maps, &device_ids);
        let caches = models
            .caches
            .into_iter()
            .map(|cache| {
                let coherence = cache
                    .section
                    .coherency_managers
                    .as_ref()
                    .map(|names| {
                        let mut map = MemoryMap::new();
                        for name in names {
                            let manager = config
                                .coherency_managers
                                .iter()
                                .flatten()
                                .find(|manager| &manager.name == name)
                                .expect("validated manager");
                            for region in memory_maps[manager.memory_map.as_str()].regions() {
                                map.insert(
                                    region.start,
                                    region.end - region.start + 1,
                                    device_ids[name],
                                )?;
                            }
                        }
                        Ok::<_, SimError>(CoherenceConfig::new(
                            device_ids[&cache.section.name],
                            Rc::new(map),
                        ))
                    })
                    .transpose()?;
                Ok(ResolvedCache {
                    name: cache.section.name.clone(),
                    config: cache.config,
                    coherence,
                })
            })
            .collect::<Result<Vec<_>, SimError>>()?;
        let pes = models
            .processing_elements
            .into_iter()
            .map(|pe| ResolvedPe {
                name: pe.section.name.clone(),
                device_id: device_ids[&pe.section.name],
                memory_map: memory_maps[pe.section.memory_map.as_str()].clone(),
                config: pe.config,
            })
            .collect();
        let memories = models
            .memories
            .into_iter()
            .map(|memory| ResolvedMemory {
                name: memory.section.name.clone(),
                config: memory.config,
            })
            .collect();
        let fabrics = models
            .fabrics
            .into_iter()
            .map(|fabric| ResolvedFabric {
                name: fabric.section.name.clone(),
                kind: fabric.section.kind,
                config: Rc::new(fabric.config.0),
                routing: fabric.config.1,
            })
            .collect();
        let mut resolved = Self {
            pes,
            memories,
            caches,
            managers,
            fabrics,
            connections: Vec::new(),
            device_names_by_id: device_ids
                .iter()
                .map(|(name, id)| (id.0, name.clone()))
                .collect(),
            reachable_memories: Vec::new(),
        };
        resolved.connections = connections
            .into_iter()
            .map(|pair| [resolved.endpoint(&pair[0]), resolved.endpoint(&pair[1])])
            .collect();
        resolved.resolve_routes(&device_ids)?;
        Ok(resolved)
    }

    fn resolve_routes(&mut self, device_ids: &DeviceIds) -> Result<(), SimError> {
        let topology = Topology::new(self);
        self.reachable_memories = (0..self.pes.len())
            .map(|index| {
                topology
                    .distances(Endpoint::Pe(index), None)
                    .into_keys()
                    .filter_map(|endpoint| match endpoint {
                        Endpoint::Memory(memory_index) => {
                            Some(device_ids[&self.memories[memory_index].name])
                        }
                        _ => None,
                    })
                    .collect()
            })
            .collect();
        for index in 0..self.fabrics.len() {
            let mut best = HashMap::<u64, (usize, Vec<usize>)>::new();
            for port in topology.fabric_ports(index) {
                for (endpoint, distance) in
                    topology.distances(Endpoint::Fabric(index, port), Some(index))
                {
                    let Some(id) = self.device_id(endpoint, device_ids) else {
                        continue;
                    };
                    let entry = best.entry(id.0).or_insert((distance, Vec::new()));
                    if distance < entry.0 {
                        *entry = (distance, Vec::new());
                    }
                    if distance == entry.0 {
                        entry.1.push(port);
                    }
                }
            }
            let ports = best
                .into_iter()
                .map(|(id, (_, mut ports))| {
                    ports.sort_unstable();
                    ports.dedup();
                    (id, ports)
                })
                .collect();
            Rc::get_mut(&mut self.fabrics[index].config)
                .expect("unshared config")
                .set_destination_port_map(ports)?;
        }
        Ok(())
    }

    fn endpoint(&self, endpoint: &ConnectionEndpointId) -> Endpoint {
        match endpoint {
            ConnectionEndpointId::Null => Endpoint::Null,
            ConnectionEndpointId::Pe { name } => Endpoint::Pe(
                self.pes
                    .iter()
                    .position(|pe| &pe.name == name)
                    .expect("validated PE"),
            ),
            ConnectionEndpointId::Mem { name } => Endpoint::Memory(
                self.memories
                    .iter()
                    .position(|memory| &memory.name == name)
                    .expect("validated memory"),
            ),
            ConnectionEndpointId::Cache { name, port } => Endpoint::Cache(
                self.caches
                    .iter()
                    .position(|cache| &cache.name == name)
                    .expect("validated cache"),
                port.expect("resolved cache port"),
            ),
            ConnectionEndpointId::CoherencyManager { name } => Endpoint::Manager(
                self.managers
                    .iter()
                    .position(|manager| &manager.0 == name)
                    .expect("validated manager"),
            ),
            ConnectionEndpointId::FabricPort {
                fabric,
                column,
                row,
                port,
            } => {
                let index = self
                    .fabrics
                    .iter()
                    .position(|value| &value.name == fabric)
                    .expect("validated fabric");
                Endpoint::Fabric(
                    index,
                    self.fabrics[index]
                        .config
                        .col_row_port_to_fabric_port_index(*column, *row, *port),
                )
            }
        }
    }

    fn device_id(&self, endpoint: Endpoint, device_ids: &DeviceIds) -> Option<DeviceId> {
        match endpoint {
            Endpoint::Pe(index) => Some(self.pes[index].device_id),
            Endpoint::Memory(index) => device_ids.get(&self.memories[index].name).copied(),
            Endpoint::Manager(index) => device_ids.get(&self.managers[index].0).copied(),
            Endpoint::Cache(index, CachePortId::Mem) => {
                device_ids.get(&self.caches[index].name).copied()
            }
            _ => None,
        }
    }
}

fn resolve_managers(
    config: &PlatformConfig,
    memory_maps: &HashMap<&str, Rc<MemoryMap>>,
    device_ids: &DeviceIds,
) -> Vec<(String, CoherencyManagerConfig)> {
    config
        .coherency_managers
        .iter()
        .flatten()
        .map(|section| {
            let mut config = CoherencyManagerConfig::new(
                section
                    .config
                    .line_size_bytes
                    .unwrap_or(DEFAULT_CACHE_LINE_SIZE_BYTES),
                device_ids[&section.name],
                memory_maps[section.memory_map.as_str()].as_ref().clone(),
            );
            if let Some(operations) = section.config.operations_per_tick {
                config = config.with_operations_per_tick(operations);
            }
            (section.name.clone(), config)
        })
        .collect::<Vec<_>>()
}

fn resolve_memory_maps<'a>(
    config: &'a PlatformConfig,
    memories: &[Configured<'_, MemorySection, MemoryConfig>],
    device_ids: &DeviceIds,
) -> Result<HashMap<&'a str, Rc<MemoryMap>>, SimError> {
    let memories_by_name = memories
        .iter()
        .map(|memory| (memory.section.name.as_str(), &memory.config))
        .collect::<HashMap<_, _>>();
    config
        .memory_maps
        .iter()
        .map(|section| {
            let mut map = MemoryMap::new();
            for name in &section.devices {
                let range = memories_by_name[name.as_str()].address_range()?;
                map.insert(
                    *range.start(),
                    *range.end() - *range.start() + 1,
                    device_ids[name],
                )?;
            }
            Ok((section.name.as_str(), Rc::new(map)))
        })
        .collect()
}

struct Topology(HashMap<Endpoint, HashSet<Endpoint>>);

impl Topology {
    fn new(platform: &ResolvedPlatform) -> Self {
        let mut edges = HashMap::<_, HashSet<_>>::new();
        for [left, right] in
            platform
                .connections
                .iter()
                .copied()
                .chain((0..platform.caches.len()).map(|index| {
                    [
                        Endpoint::Cache(index, CachePortId::Dev),
                        Endpoint::Cache(index, CachePortId::Mem),
                    ]
                }))
        {
            if left == Endpoint::Null || right == Endpoint::Null {
                continue;
            }
            edges.entry(left).or_default().insert(right);
            edges.entry(right).or_default().insert(left);
        }
        Self(edges)
    }

    fn distances(
        &self,
        start: Endpoint,
        excluded_fabric: Option<usize>,
    ) -> HashMap<Endpoint, usize> {
        let mut visited = HashMap::from([(start, 0)]);
        let mut queue = VecDeque::from([start]);
        while let Some(node) = queue.pop_front() {
            let distance = visited[&node] + 1;
            let mut neighbors = self
                .0
                .get(&node)
                .into_iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>();
            if let Endpoint::Fabric(index, _) = node
                && Some(index) != excluded_fabric
            {
                neighbors.extend(
                    self.fabric_ports(index)
                        .map(|port| Endpoint::Fabric(index, port)),
                );
            }
            for neighbor in neighbors {
                if let Entry::Vacant(entry) = visited.entry(neighbor) {
                    entry.insert(distance);
                    queue.push_back(neighbor);
                }
            }
        }
        visited
    }

    fn fabric_ports(&self, fabric: usize) -> impl Iterator<Item = usize> + '_ {
        self.0.keys().filter_map(move |endpoint| match endpoint {
            Endpoint::Fabric(index, port) if *index == fabric => Some(*port),
            _ => None,
        })
    }
}
