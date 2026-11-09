// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Address-aware roofline routes without model or engine registration.
//! Footprint address validation remains the caller's job.

#[cfg(test)]
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gwr_engine::types::SimError;
use gwr_models::fabric::FabricPortSelection;
use gwr_models::fabric::node::FabricRoutingAlgorithm;
use gwr_models::fabric::paths::{FabricAddressKey, FabricPathSelection, FabricPathSelector};
use gwr_models::roofline::accounting::{Operation, TransactionRun};
use gwr_models::roofline::plan::{DemandKind, ResourceUse, ServicePlan};

use crate::DeviceIds;
use crate::builder::EffectiveConfigs;
use crate::connection_id::{CachePortId, ConnectionEndpointId, parse_connection_endpoint_id};
use crate::roofline_resources::{ResourceCatalog, ResourceKind, delivery_kind, link_kind};
use crate::types::{FabricKind, PlatformConfig};
use crate::validation::cache_connection_port;

mod cache;
#[cfg(test)]
pub(crate) use cache::CompiledReadPath;

pub(crate) struct CacheResourceUses {
    pub request: Vec<ResourceUse>,
    pub response: Vec<ResourceUse>,
    pub resource_count: usize,
}

/// Reusable path shape. Request and response routing are independent. Submit
/// plans only with the resource catalog from the compiler that produced this
/// route. This is not a footprint/transaction provenance ledger.
#[derive(Clone, Debug)]
pub struct CompiledRoute {
    operation: Operation,
    uses: Vec<ResourceUse>,
    resource_count: usize,
    latency: u64,
    #[cfg(test)]
    decisions: Vec<RouteDecision>,
}

/// Immutable aggregate resource demand for a cache-aware route, with all
/// cache reads conservatively modelled as misses. Resource indices belong to
/// the compiler's [`ResourceCatalog`].
#[derive(Clone, Debug)]
pub struct AggregateRouteService {
    pub resource_demands: Vec<(usize, u128)>,
    pub cache_stages: usize,
}

/// Expected aggregate route demand, uniformly averaged over the finite
/// address-selector period. This avoids expanding individual transaction
/// addresses when footprint summaries no longer retain address order.
#[derive(Clone, Debug)]
pub struct AverageRouteService {
    pub resource_demands: Vec<(usize, f64)>,
    pub cache_stages: usize,
    pub sampled_offsets: usize,
}

impl CompiledRoute {
    pub fn plan(&self, runs: &[TransactionRun]) -> Result<ServicePlan, SimError> {
        ServicePlan::new(
            self.operation,
            runs,
            &self.uses,
            self.resource_count,
            &[self.latency],
        )
    }

    fn add_latency(&mut self, ticks: u64) -> Result<(), SimError> {
        self.latency = self
            .latency
            .checked_add(ticks)
            .ok_or_else(|| SimError("roofline route latency overflow".into()))?;
        Ok(())
    }
}

/// Built from immutable validated preflight state. Compilation walks hardware
/// paths, not transactions. Cache/detailed boundaries are deliberately
/// rejected.
pub struct RooflineRoutes<'a> {
    cfg: &'a PlatformConfig,
    effective: &'a EffectiveConfigs,
    device_ids: &'a DeviceIds,
    resources: Rc<ResourceCatalog>,
    neighbors: Rc<BTreeMap<String, ConnectionEndpointId>>,
    indices: Rc<SectionIndices>,
}

impl<'a> RooflineRoutes<'a> {
    pub(crate) fn new(
        cfg: &'a PlatformConfig,
        effective: &'a EffectiveConfigs,
        device_ids: &'a DeviceIds,
    ) -> Result<Self, SimError> {
        let mut neighbors = BTreeMap::new();
        for connection in cfg.connections.iter().flatten() {
            let a = parse_connection_endpoint_id(&connection.connect[0])?;
            let b = parse_connection_endpoint_id(&connection.connect[1])?;
            let left = normalize(&a, &b, true);
            let right = normalize(&b, &a, false);
            neighbors.insert(left.to_string(), right.clone());
            neighbors.insert(right.to_string(), left);
        }
        Ok(Self {
            cfg,
            effective,
            device_ids,
            resources: Rc::new(ResourceCatalog::from_configs(cfg, effective)?),
            neighbors: Rc::new(neighbors),
            indices: Rc::new(SectionIndices::new(cfg)),
        })
    }

    #[must_use]
    pub fn resources(&self) -> &ResourceCatalog {
        &self.resources
    }

    pub fn compile(
        &self,
        pe_name: &str,
        memory_name: &str,
        operation: Operation,
    ) -> Result<CompiledRoute, SimError> {
        self.compile_with_key(pe_name, memory_name, operation, None)
    }

    /// Compile one physical address using the native routed fabric's canonical
    /// memory ordinal, attachment hash, and independent plane selector.
    pub fn compile_for_address(
        &self,
        pe_name: &str,
        memory_name: &str,
        operation: Operation,
        offset_in_memory: u64,
    ) -> Result<CompiledRoute, SimError> {
        self.compile_with_key(
            pe_name,
            memory_name,
            operation,
            Some(self.address_key(memory_name, offset_in_memory)?),
        )
    }

    /// Aggregate one physical route into resource byte demands. Read traffic
    /// includes request/response service at every cache and the complete
    /// downstream all-miss path. Writes use the path's native write plan.
    /// Striped traffic should be submitted once per physical destination and
    /// with its canonical address offset.
    pub fn plan_all_miss(
        &self,
        pe_name: &str,
        memory_name: &str,
        operation: Operation,
        offset_in_memory: u64,
        runs: &[TransactionRun],
    ) -> Result<AggregateRouteService, SimError> {
        let path = self.compile_path_for_address(pe_name, memory_name, offset_in_memory)?;
        let mut demands = BTreeMap::<usize, u128>::new();
        let mut add_plan = |plan: ServicePlan| -> Result<(), SimError> {
            for (resource, bytes) in plan.resource_demands() {
                let demand = demands.entry(*resource).or_default();
                *demand = demand.checked_add(*bytes).ok_or_else(|| {
                    SimError(format!(
                        "aggregate route demand overflow on resource {resource}"
                    ))
                })?;
            }
            Ok(())
        };
        match operation {
            Operation::Read => {
                for cache in &path.caches {
                    let traffic = gwr_models::roofline::accounting::account(operation, runs)
                        .map_err(|error| {
                            SimError(format!("cache traffic accounting: {error:?}"))
                        })?;
                    if traffic.request_wire > 0 && !cache.resources.request.is_empty() {
                        add_plan(
                            ServicePlan::new(
                                Operation::Read,
                                runs,
                                &cache.resources.request,
                                cache.resources.resource_count,
                                &[0],
                            )
                            .map_err(|error| SimError(format!("cache request service: {error}")))?,
                        )?;
                    }
                    if traffic.response_wire > 0 && !cache.resources.response.is_empty() {
                        add_plan(
                            ServicePlan::new(
                                Operation::Read,
                                runs,
                                &cache.resources.response,
                                cache.resources.resource_count,
                                &[0],
                            )
                            .map_err(|error| {
                                SimError(format!("cache response service: {error}"))
                            })?,
                        )?;
                    }
                }
                add_plan(
                    path.memory
                        .plan(runs)
                        .map_err(|error| SimError(format!("downstream miss service: {error}")))?,
                )?;
            }
            Operation::WriteNonPosted | Operation::WritePosted => {
                add_plan(
                    path.write
                        .plan(runs)
                        .map_err(|error| SimError(format!("downstream write service: {error}")))?,
                )?;
            }
        }
        Ok(AggregateRouteService {
            resource_demands: demands.into_iter().collect(),
            cache_stages: path.caches.len(),
        })
    }

    /// Average aggregate demand over the full address-xor selector period.
    /// Each selector class receives equal weight; this preserves multi-plane
    /// and gateway utilization without walking every tensor address. Mix-modulo
    /// selectors use a bounded 4096-line sample because they have no short
    /// period guarantee. Periods above 4096 samples fail explicitly.
    pub fn plan_uniform_all_miss(
        &self,
        pe_name: &str,
        memory_name: &str,
        operation: Operation,
        runs: &[TransactionRun],
    ) -> Result<AverageRouteService, SimError> {
        let routed: Vec<_> = self
            .cfg
            .fabrics
            .iter()
            .flatten()
            .filter(|fabric| matches!(fabric.kind, FabricKind::Routed))
            .filter_map(|fabric| fabric.config.paths.as_ref())
            .collect();
        let min_shift = routed
            .iter()
            .map(|paths| paths.selection.address_shift())
            .min()
            .unwrap_or(0);
        let max_period_bits = routed
            .iter()
            .map(|paths| match paths.selection {
                FabricPathSelection::AddressXorFold { address_shift } => {
                    address_shift as u32 + paths.count.ilog2()
                }
                FabricPathSelection::AddressMixModulo { address_shift } => {
                    address_shift as u32 + 12
                }
            })
            .max()
            .unwrap_or(min_shift.into());
        let sample_count = 1usize
            .checked_shl(max_period_bits.saturating_sub(min_shift.into()))
            .unwrap_or(usize::MAX);
        if sample_count > 4096 {
            return Err(SimError(format!(
                "routed address selector period {sample_count} exceeds analytical sampling limit 4096"
            )));
        }
        let sample_count = sample_count.max(1);
        let mut demands = BTreeMap::<usize, f64>::new();
        let mut cache_stages = 0;
        for offset in 0..sample_count {
            let service = self.plan_all_miss(
                pe_name,
                memory_name,
                operation,
                (offset as u64) << min_shift,
                runs,
            )?;
            cache_stages = cache_stages.max(service.cache_stages);
            for (resource, bytes) in service.resource_demands {
                *demands.entry(resource).or_default() += bytes as f64 / sample_count as f64;
            }
        }
        Ok(AverageRouteService {
            resource_demands: demands.into_iter().collect(),
            cache_stages,
            sampled_offsets: sample_count,
        })
    }

    fn address_key(
        &self,
        memory: &str,
        offset_in_memory: u64,
    ) -> Result<FabricAddressKey, SimError> {
        let &index = self
            .indices
            .memories
            .get(memory)
            .ok_or_else(|| SimError(format!("Unknown memory '{memory}'")))?;
        Ok(FabricAddressKey {
            memory_index: index as u64,
            offset_in_memory,
        })
    }

    fn compile_with_key(
        &self,
        pe_name: &str,
        memory_name: &str,
        operation: Operation,
        key: Option<FabricAddressKey>,
    ) -> Result<CompiledRoute, SimError> {
        let pe_index = *self
            .indices
            .pes
            .get(pe_name)
            .ok_or_else(|| SimError(format!("Unknown PE '{pe_name}'")))?;
        let pe = &self.cfg.processing_elements.as_ref().unwrap()[pe_index];
        let memory_index = *self
            .indices
            .memories
            .get(memory_name)
            .ok_or_else(|| SimError(format!("Unknown memory '{memory_name}'")))?;
        if self
            .resources
            .index(memory_name, ResourceKind::MemoryPayload)
            .is_none()
        {
            return Err(SimError(
                "roofline route has no memory payload resource".into(),
            ));
        }
        if !self.cfg.memory_maps.iter().any(|map| {
            map.name == pe.memory_map && map.devices.iter().any(|device| device == memory_name)
        }) {
            return Err(SimError(format!(
                "Memory '{memory_name}' is not mapped for PE '{pe_name}'"
            )));
        }
        let mut route = CompiledRoute {
            operation,
            uses: Vec::new(),
            resource_count: self.resources.resources().len(),
            latency: 0,
            #[cfg(test)]
            decisions: Vec::new(),
        };
        self.charge(
            &mut route,
            memory_name,
            ResourceKind::MemoryPayload,
            DemandKind::Payload,
        )?;
        route.add_latency(ticks(self.effective.memories[memory_index].delay_ticks())?)?;
        let pe = ConnectionEndpointId::Pe {
            name: pe_name.into(),
        };
        let memory = ConnectionEndpointId::Mem {
            name: memory_name.into(),
        };
        self.walk(
            &mut route,
            &pe,
            &memory,
            self.device_ids[pe_name].0,
            self.device_ids[memory_name].0,
            DemandKind::RequestWire,
            key,
        )?;
        if operation != Operation::WritePosted {
            self.walk(
                &mut route,
                &memory,
                &pe,
                self.device_ids[memory_name].0,
                self.device_ids[pe_name].0,
                DemandKind::ResponseWire,
                key,
            )?;
        }
        Ok(route)
    }

    fn charge(
        &self,
        route: &mut CompiledRoute,
        name: &str,
        kind: ResourceKind,
        demand: DemandKind,
    ) -> Result<(), SimError> {
        let resource = self.resources.index(name, kind).ok_or_else(|| {
            SimError(format!(
                "Missing roofline resource '{name}' {kind:?}; detailed boundaries need adapters"
            ))
        })?;
        route.uses.push(ResourceUse {
            resource,
            kind: demand,
        });
        Ok(())
    }

    fn select(
        &self,
        spec: DecisionSpec,
        key: Option<FabricAddressKey>,
    ) -> Result<(usize, usize), SimError> {
        let section = &self.cfg.fabrics.as_ref().unwrap()[spec.fabric];
        let config = &self.effective.fabrics[spec.fabric].0;
        let paths = section.config.paths.clone().unwrap_or_default();
        if matches!(section.kind, FabricKind::Routed) {
            if let Some(key) = key {
                return Ok((
                    paths.selection.select(key, paths.count)?,
                    config.resolve_canonical_destination_port(
                        spec.destination_id,
                        key,
                        paths.selection.address_shift(),
                    )?,
                ));
            }
            if paths.count != 1 {
                return Err(SimError(format!(
                    "Fabric '{}' address-selected paths require a physical address; transaction-ledger splitting is required",
                    section.name
                )));
            }
        }
        let candidates = config
            .destination_port_map()
            .get(&spec.destination_id)
            .ok_or_else(|| {
                SimError(format!(
                    "Fabric '{}' has no route to device {}",
                    section.name, spec.destination_id
                ))
            })?;
        let egress = match candidates.as_slice() {
            [only] => *only,
            [] => {
                return Err(SimError(format!(
                    "Fabric '{}' has no candidate egress",
                    section.name
                )));
            }
            ports
                if matches!(section.kind, FabricKind::Functional)
                    && config.port_selection() == FabricPortSelection::SourceIdModulo =>
            {
                ports[(spec.source_id as usize) % ports.len()]
            }
            _ => {
                return Err(SimError(format!(
                    "Fabric '{}' multi-candidate address hashing requires transaction-ledger splitting and a physical address",
                    section.name
                )));
            }
        };
        Ok((0, egress))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Route walking needs both endpoints and transaction routing metadata"
    )]
    fn walk(
        &self,
        route: &mut CompiledRoute,
        start: &ConnectionEndpointId,
        target: &ConnectionEndpointId,
        source_id: u64,
        destination_id: u64,
        demand: DemandKind,
        key: Option<FabricAddressKey>,
    ) -> Result<(), SimError> {
        let reached =
            self.walk_segment(route, start, target, source_id, destination_id, demand, key)?;
        if &reached != target {
            return Err(SimError(format!(
                "Roofline route reached '{reached}'; caches require a cache plan"
            )));
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "Keep route traversal and per-hop validation together with transaction metadata"
    )]
    fn walk_segment(
        &self,
        route: &mut CompiledRoute,
        start: &ConnectionEndpointId,
        target: &ConnectionEndpointId,
        source_id: u64,
        destination_id: u64,
        demand: DemandKind,
        key: Option<FabricAddressKey>,
    ) -> Result<ConnectionEndpointId, SimError> {
        let mut current = start.clone();
        let mut visited = BTreeSet::new();
        loop {
            if !visited.insert(current.to_string()) {
                return Err(SimError(format!(
                    "Non-terminating roofline route at '{current}'"
                )));
            }
            let next = self
                .neighbors
                .get(&current.to_string())
                .ok_or_else(|| SimError(format!("Missing connection at '{current}'")))?;
            if next == target {
                return Ok(next.clone());
            }
            if matches!(next, ConnectionEndpointId::Cache { .. }) {
                return Ok(next.clone());
            }
            let ConnectionEndpointId::FabricPort {
                fabric,
                column,
                row,
                port,
            } = next
            else {
                return Err(SimError(format!(
                    "Roofline route reached unsupported or wrong endpoint '{next}' before '{target}'; caches and boundaries need separate plans"
                )));
            };
            let index = *self
                .indices
                .fabrics
                .get(fabric)
                .ok_or_else(|| SimError(format!("Unknown fabric '{fabric}'")))?;
            let section = &self.cfg.fabrics.as_ref().expect("resolved fabric")[index];
            let (config, algorithm) = &self.effective.fabrics[index];
            let ingress = config.col_row_port_to_fabric_port_index(*column, *row, *port);
            let spec = DecisionSpec {
                fabric: index,
                source_id,
                destination_id,
            };
            let (plane, egress) = self.select(spec, key)?;
            #[cfg(test)]
            route.decisions.push(RouteDecision {
                spec,
                selected: (plane, egress),
            });
            let paths = section.config.paths.clone().unwrap_or_default();
            if ingress == egress {
                return Err(SimError(format!(
                    "Fabric '{fabric}' route selects its own ingress {ingress}"
                )));
            }
            if matches!(section.kind, FabricKind::Functional) {
                self.charge(
                    route,
                    fabric,
                    ResourceKind::FabricIngress { port: ingress },
                    demand,
                )?;
                self.charge(
                    route,
                    fabric,
                    ResourceKind::FabricEgress { port: egress },
                    demand,
                )?;
            } else {
                self.charge(
                    route,
                    fabric,
                    delivery_kind(paths.count, plane, egress),
                    demand,
                )?;
            }
            let (end_column, end_row, end_port) = config.fabric_port_index_to_col_row_port(egress);
            let hops = column
                .abs_diff(end_column)
                .checked_add(row.abs_diff(end_row))
                .ok_or_else(|| SimError("roofline route hop count overflow".into()))?;
            let delay = ticks(hops)?
                .checked_mul(ticks(config.ticks_per_hop())?)
                .ok_or_else(|| SimError("roofline route latency overflow".into()))?;
            route.add_latency(delay)?;
            route.add_latency(ticks(config.ticks_overhead())?)?;
            if matches!(section.kind, FabricKind::Routed) {
                let mut from = (*column, *row);
                let end = (end_column, end_row);
                while from != end {
                    let mut to = from;
                    if from.0 != end.0
                        && (from.1 == end.1 || *algorithm == FabricRoutingAlgorithm::ColumnFirst)
                    {
                        to.0 = if from.0 < end.0 {
                            from.0 + 1
                        } else {
                            from.0 - 1
                        };
                    } else {
                        to.1 = if from.1 < end.1 {
                            from.1 + 1
                        } else {
                            from.1 - 1
                        };
                    }
                    self.charge(
                        route,
                        fabric,
                        link_kind(paths.count, plane, from, to),
                        demand,
                    )?;
                    from = to;
                }
            }
            current = ConnectionEndpointId::FabricPort {
                fabric: fabric.clone(),
                column: end_column,
                row: end_row,
                port: end_port,
            };
        }
    }
}

fn ticks(value: usize) -> Result<u64, SimError> {
    u64::try_from(value).map_err(|_| SimError("roofline latency does not fit u64 ticks".into()))
}

fn normalize(
    endpoint: &ConnectionEndpointId,
    other: &ConnectionEndpointId,
    is_from: bool,
) -> ConnectionEndpointId {
    match endpoint {
        ConnectionEndpointId::Cache { name, port } => ConnectionEndpointId::Cache {
            name: name.clone(),
            port: Some(
                if cache_connection_port(port.as_ref(), other, is_from) == "dev" {
                    CachePortId::Dev
                } else {
                    CachePortId::Mem
                },
            ),
        },
        _ => endpoint.clone(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DecisionSpec {
    fabric: usize,
    source_id: u64,
    destination_id: u64,
}

#[cfg(test)]
#[derive(Clone, Debug)]
struct RouteDecision {
    spec: DecisionSpec,
    selected: (usize, usize),
}

struct SectionIndices {
    pes: BTreeMap<String, usize>,
    memories: BTreeMap<String, usize>,
    fabrics: BTreeMap<String, usize>,
    caches: BTreeMap<String, usize>,
}

impl SectionIndices {
    fn new(cfg: &PlatformConfig) -> Self {
        Self {
            pes: cfg
                .processing_elements
                .iter()
                .flatten()
                .enumerate()
                .map(|(i, s)| (s.name.clone(), i))
                .collect(),
            memories: cfg
                .memories
                .iter()
                .flatten()
                .enumerate()
                .map(|(i, s)| (s.name.clone(), i))
                .collect(),
            fabrics: cfg
                .fabrics
                .iter()
                .flatten()
                .enumerate()
                .map(|(i, s)| (s.name.clone(), i))
                .collect(),
            caches: cfg
                .caches
                .iter()
                .flatten()
                .enumerate()
                .map(|(i, s)| (s.name.clone(), i))
                .collect(),
        }
    }
}

// A trie tests only decisions actually visited by a route. Unrelated fabric
// attachments never multiply the number of cached paths. Keys contain selected
// hardware services, never physical offsets or affinity-region numbers.
#[cfg(test)]
#[derive(Default)]
enum RouteTrie {
    #[default]
    Empty,
    Branch {
        spec: DecisionSpec,
        children: BTreeMap<(usize, usize), RouteTrie>,
    },
    Path(Rc<CompiledReadPath>),
}

#[cfg(test)]
impl RouteTrie {
    fn find(
        &self,
        compiler: &RooflineRoutes<'_>,
        key: FabricAddressKey,
    ) -> Result<Option<Rc<CompiledReadPath>>, SimError> {
        match self {
            Self::Empty => Ok(None),
            Self::Path(path) => Ok(Some(path.clone())),
            Self::Branch { spec, children } => {
                match children.get(&compiler.select(*spec, Some(key))?) {
                    Some(child) => child.find(compiler, key),
                    None => Ok(None),
                }
            }
        }
    }

    fn insert(
        &mut self,
        decisions: &[RouteDecision],
        path: Rc<CompiledReadPath>,
    ) -> Result<(), SimError> {
        let Some((decision, rest)) = decisions.split_first() else {
            *self = Self::Path(path);
            return Ok(());
        };
        if matches!(self, Self::Empty) {
            *self = Self::Branch {
                spec: decision.spec,
                children: BTreeMap::new(),
            };
        }
        let Self::Branch { spec, children } = self else {
            return Err(SimError(
                "inconsistent roofline route decision prefix".into(),
            ));
        };
        if *spec != decision.spec {
            return Err(SimError(
                "inconsistent roofline route decision order".into(),
            ));
        }
        children
            .entry(decision.selected)
            .or_default()
            .insert(rest, path)
    }
}

/// Lifetime-independent, reusable address-aware compiler. Configuration,
/// resources and static neighbors are prepared once; paths are cached by the
/// actual selected planes and attachments, rather than by physical address.
#[cfg(test)]
pub struct OwnedRooflineRoutes {
    cfg: PlatformConfig,
    effective: EffectiveConfigs,
    device_ids: DeviceIds,
    resources: Rc<ResourceCatalog>,
    neighbors: Rc<BTreeMap<String, ConnectionEndpointId>>,
    indices: Rc<SectionIndices>,
    paths: RefCell<BTreeMap<(usize, usize), RouteTrie>>,
}

#[cfg(test)]
impl OwnedRooflineRoutes {
    /// Validate and retain an owned platform snapshot without activating
    /// models.
    pub fn new(cfg: &PlatformConfig) -> Result<Self, SimError> {
        let mut effective = cfg.validated()?;
        let device_ids = crate::assign_device_ids(cfg)?;
        let destinations = crate::builder::build_fabric_destination_port_maps(cfg, &device_ids)?;
        effective.fabrics =
            crate::builder::fabric_configs_with_destination_port_maps(cfg, &destinations)?;
        Self::from_prepared(cfg, &effective, &device_ids)
    }

    /// Retain a snapshot of already validated, fully routed preflight state.
    /// Runtime construction uses this entry point to avoid repeating topology
    /// validation and destination-map graph traversal.
    pub(crate) fn from_prepared(
        cfg: &PlatformConfig,
        effective: &EffectiveConfigs,
        device_ids: &DeviceIds,
    ) -> Result<Self, SimError> {
        let compiler = RooflineRoutes::new(cfg, effective, device_ids)?;
        let resources = compiler.resources.clone();
        let neighbors = compiler.neighbors.clone();
        let indices = compiler.indices.clone();
        Ok(Self {
            cfg: cfg.clone(),
            effective: EffectiveConfigs {
                processing_elements: effective.processing_elements.clone(),
                caches: effective.caches.clone(),
                fabrics: effective.fabrics.clone(),
                memories: effective.memories.clone(),
            },
            device_ids: device_ids.clone(),
            resources,
            neighbors,
            indices,
            paths: RefCell::new(BTreeMap::new()),
        })
    }

    fn compiler(&self) -> RooflineRoutes<'_> {
        RooflineRoutes {
            cfg: &self.cfg,
            effective: &self.effective,
            device_ids: &self.device_ids,
            resources: self.resources.clone(),
            neighbors: self.neighbors.clone(),
            indices: self.indices.clone(),
        }
    }

    /// Resource capacities shared by every returned path.
    #[must_use]
    pub fn resources(&self) -> &ResourceCatalog {
        &self.resources
    }

    /// Whether any fabric requires canonical physical address selection.
    #[must_use]
    pub fn uses_address_routing(&self) -> bool {
        self.cfg
            .fabrics
            .iter()
            .flatten()
            .any(|f| matches!(f.kind, FabricKind::Routed))
    }

    /// Smallest physical address region on which all routed selectors are
    /// constant. Split a physical span at these aligned boundaries before
    /// grouping its traffic by the returned path. No routed fabric gives MAX.
    #[must_use]
    pub fn affinity_bytes(&self) -> u64 {
        self.cfg
            .fabrics
            .iter()
            .flatten()
            .filter(|f| matches!(f.kind, FabricKind::Routed))
            .map(|f| {
                1u64 << f
                    .config
                    .paths
                    .clone()
                    .unwrap_or_default()
                    .selection
                    .address_shift()
            })
            .min()
            .unwrap_or(u64::MAX)
    }

    /// Resolve a reusable cache/read/write path for a physical memory offset.
    /// Cache hits evaluate only the fabric decisions on that route.
    pub(crate) fn path(
        &self,
        pe: &str,
        memory: &str,
        offset: u64,
    ) -> Result<Rc<CompiledReadPath>, SimError> {
        let compiler = self.compiler();
        let key = compiler.address_key(memory, offset)?;
        let pe_index = *self
            .indices
            .pes
            .get(pe)
            .ok_or_else(|| SimError(format!("Unknown PE '{pe}'")))?;
        let pair = (pe_index, key.memory_index as usize);
        let mut paths = self.paths.borrow_mut();
        let trie = paths.entry(pair).or_default();
        if let Some(path) = trie.find(&compiler, key)? {
            return Ok(path);
        }
        let path = Rc::new(compiler.compile_path_for_address(pe, memory, offset)?);
        trie.insert(&path.write.decisions, path.clone())?;
        Ok(path)
    }
}

#[cfg(test)]
mod affinity_tests {
    use gwr_models::roofline::accounting::TransactionRun;

    use super::*;

    fn config() -> PlatformConfig {
        yaml_serde::from_str(r"
operation_classes: {binary: {operations: [add], arity: 2}}
memory_maps: [{name: mm, devices: [local, remote]}]
processing_elements:
  - name: pe
    memory_map: mm
    config: {compute: {engines: {vector: {capabilities: [{class: binary, ops_per_tick: 1}]}}}}
memories:
  - {name: local, kind: hbm, base_address: 0, config: {capacity_bytes: 4096}}
  - {name: remote, kind: hbm, base_address: 4096, config: {capacity_bytes: 4096}}
fabrics:
  - name: left
    kind: routed
    columns: 3
    rows: 1
    config:
      fabric_ports_per_node: 2
      ticks_per_hop: 2
      ticks_overhead: 3
      paths: {count: 4, bw_bits_per_tick: 435, selection: {policy: address-xor-fold-v1, address_shift: 6}, queue_bytes: 8192, queue_packets: 256}
  - name: right
    kind: routed
    columns: 3
    rows: 1
    config:
      fabric_ports_per_node: 2
      ticks_per_hop: 2
      ticks_overhead: 3
      paths: {count: 4, bw_bits_per_tick: 435, selection: {policy: address-xor-fold-v1, address_shift: 6}, queue_bytes: 8192, queue_packets: 256}
connections:
  - connect: [pe.pe, 'fabric.left@(0,0).0']
  - connect: [mem.local, 'fabric.left@(0,0).1']
  - connect: ['fabric.left@(1,0).0', 'fabric.right@(0,0).0']
  - connect: ['fabric.left@(2,0).0', 'fabric.right@(1,0).0']
  - connect: [mem.remote, 'fabric.right@(2,0).0']
").unwrap()
    }

    #[test]
    fn canonical_physical_offset_selects_native_planes_and_gateways_in_both_directions() {
        let cfg = config();
        let owned = OwnedRooflineRoutes::new(&cfg).unwrap();
        let compiler = owned.compiler();
        let mut planes = BTreeSet::new();
        let mut gateways = BTreeSet::new();
        assert_eq!(owned.affinity_bytes(), 64);
        assert!(compiler.compile("pe", "remote", Operation::Read).is_err());
        for offset in (0..4096).step_by(64) {
            // Final memory's ordinal is 1; runtime device ID is 2.
            let key = FabricAddressKey {
                memory_index: 1,
                offset_in_memory: offset,
            };
            assert_eq!(compiler.address_key("remote", offset).unwrap(), key);
            let route = compiler
                .compile_for_address("pe", "remote", Operation::Read, offset)
                .unwrap();
            assert_eq!(route.decisions.len(), 4);
            for decision in &route.decisions {
                let fabric = &cfg.fabrics.as_ref().unwrap()[decision.spec.fabric];
                let paths = fabric.config.paths.as_ref().unwrap();
                let native = &owned.effective.fabrics[decision.spec.fabric].0;
                let plane = paths.selection.select(key, paths.count).unwrap();
                let port = native
                    .resolve_canonical_destination_port(
                        decision.spec.destination_id,
                        key,
                        paths.selection.address_shift(),
                    )
                    .unwrap();
                assert_eq!(decision.selected, (plane, port));
                planes.insert(plane);
            }
            gateways.insert(route.decisions[0].selected.1);
            let path = owned.path("pe", "remote", offset).unwrap();
            let within_region = owned.path("pe", "remote", offset + 63).unwrap();
            assert!(Rc::ptr_eq(&path, &within_region));
            let run = [TransactionRun {
                payload_bytes: 64,
                count: 1,
                request_header_bytes: 8,
                response_header_bytes: 8,
            }];
            assert_eq!(
                path.memory.plan(&run).unwrap().resource_demands(),
                route.plan(&run).unwrap().resource_demands()
            );
        }
        assert_eq!(planes.len(), 4);
        assert_eq!(gateways.len(), 2);
    }

    fn leaf_count(trie: &RouteTrie) -> usize {
        match trie {
            RouteTrie::Empty => 0,
            RouteTrie::Path(_) => 1,
            RouteTrie::Branch { children, .. } => children.values().map(leaf_count).sum(),
        }
    }

    #[test]
    fn local_route_cache_ignores_unused_mesh_gateways_and_reuses_shapes() {
        let owned = OwnedRooflineRoutes::new(&config()).unwrap();
        let mut pointers = BTreeSet::new();
        for offset in (0..1_048_576).step_by(64) {
            let path = owned.path("pe", "local", offset).unwrap();
            assert_eq!(path.write.decisions.len(), 2);
            assert!(path.write.decisions.iter().all(|d| d.spec.fabric == 0));
            pointers.insert(Rc::as_ptr(&path) as usize);
        }
        assert_eq!(pointers.len(), 4);
        let cache = owned.paths.borrow();
        assert_eq!(leaf_count(&cache[&(0, 0)]), 4);
    }

    #[test]
    fn public_aggregate_service_plan_matches_plain_route_demands() {
        let owned = OwnedRooflineRoutes::new(&config()).unwrap();
        let run = [TransactionRun {
            payload_bytes: 64,
            count: 1,
            request_header_bytes: 8,
            response_header_bytes: 8,
        }];
        let compiler = owned.compiler();
        let plan = compiler
            .plan_all_miss("pe", "remote", Operation::Read, 128, &run)
            .unwrap();
        assert_eq!(
            plan.resource_demands,
            compiler
                .compile_path_for_address("pe", "remote", 128)
                .unwrap()
                .memory
                .plan(&run)
                .unwrap()
                .resource_demands()
        );
    }

    #[test]
    fn aggregate_read_service_includes_cache_and_downstream_miss_resources() {
        let cfg: PlatformConfig = yaml_serde::from_str(
            r"
operation_classes: {binary: {operations: [add], arity: 2}}
memory_maps: [{name: mm, devices: [mem]}]
processing_elements:
  - name: pe
    memory_map: mm
    config: {compute: {engines: {vector: {capabilities: [{class: binary, ops_per_tick: 1}]}}}}
caches:
  - name: cache
    config: {bw_bytes_per_tick: 16, line_size_bytes: 64, num_ways: 1, num_sets: 8, delay_ticks: 1}
memories:
  - name: mem
    kind: hbm
    base_address: 0
    config: {capacity_bytes: 4096, bw_bytes_per_tick: 16}
fabrics:
  - name: fabric
    kind: functional
    columns: 2
    rows: 1
    config: {routing: column-first, port_bits_per_tick: 128}
connections:
  - connect: [pe.pe, cache.cache.dev]
  - connect: [cache.cache.mem, 'fabric.fabric@(0,0)']
  - connect: [mem.mem, 'fabric.fabric@(1,0)']
",
        )
        .unwrap();
        let owned = OwnedRooflineRoutes::new(&cfg).unwrap();
        let compiler = owned.compiler();
        let runs = [TransactionRun {
            payload_bytes: 64,
            count: 1,
            request_header_bytes: 8,
            response_header_bytes: 8,
        }];
        let plan = compiler
            .plan_all_miss("pe", "mem", Operation::Read, 0, &runs)
            .unwrap();
        let cache_request = owned
            .resources
            .index("cache", ResourceKind::CacheRequest)
            .unwrap();
        let cache_response = owned
            .resources
            .index("cache", ResourceKind::CacheMemoryResponse)
            .unwrap();
        assert_eq!(plan.cache_stages, 1);
        assert!(
            plan.resource_demands
                .iter()
                .any(|(resource, _)| *resource == cache_request)
        );
        assert!(
            plan.resource_demands
                .iter()
                .any(|(resource, _)| *resource == cache_response)
        );
    }

    #[test]
    fn zero_header_read_skips_empty_cache_request_service() {
        let cfg: PlatformConfig = yaml_serde::from_str(
            r"
operation_classes: {binary: {operations: [add], arity: 2}}
memory_maps: [{name: mm, devices: [mem]}]
processing_elements:
  - name: pe
    memory_map: mm
    config: {compute: {engines: {vector: {capabilities: [{class: binary, ops_per_tick: 1}]}}}}
caches:
  - name: cache
    config: {bw_bytes_per_tick: 16, line_size_bytes: 64, num_ways: 1, num_sets: 8, delay_ticks: 1}
memories:
  - name: mem
    kind: hbm
    base_address: 0
    config: {capacity_bytes: 4096, bw_bytes_per_tick: 16}
connections:
  - connect: [pe.pe, cache.cache.dev]
  - connect: [cache.cache.mem, mem.mem]
",
        )
        .unwrap();
        let owned = OwnedRooflineRoutes::new(&cfg).unwrap();
        let plan = owned
            .compiler()
            .plan_all_miss(
                "pe",
                "mem",
                Operation::Read,
                0,
                &[TransactionRun {
                    payload_bytes: 64,
                    count: 1,
                    request_header_bytes: 0,
                    response_header_bytes: 0,
                }],
            )
            .unwrap();
        assert!(plan.resource_demands.iter().all(|(_, bytes)| *bytes > 0));
    }

    #[test]
    fn uniform_address_sampling_charges_all_selected_fabric_planes() {
        let cfg = config();
        let owned = OwnedRooflineRoutes::new(&cfg).unwrap();
        let service = owned
            .compiler()
            .plan_uniform_all_miss(
                "pe",
                "remote",
                Operation::Read,
                &[TransactionRun {
                    payload_bytes: 64,
                    count: 100,
                    request_header_bytes: 8,
                    response_header_bytes: 8,
                }],
            )
            .unwrap();
        let mut planes = BTreeSet::new();
        for (resource, bytes) in &service.resource_demands {
            if *bytes == 0. {
                continue;
            }
            match owned.resources.resources()[*resource].kind {
                ResourceKind::FabricPlaneDelivery { plane, .. }
                | ResourceKind::FabricPlaneLink { plane, .. } => {
                    planes.insert(plane);
                }
                _ => {}
            }
        }
        assert_eq!(service.sampled_offsets, 4);
        assert_eq!(planes, BTreeSet::from([0, 1, 2, 3]));
    }

    #[test]
    fn single_path_routed_fabric_needs_only_one_sample() {
        let mut cfg = config();
        for fabric in cfg.fabrics.as_mut().unwrap() {
            fabric.config.paths.as_mut().unwrap().count = 1;
        }
        let owned = OwnedRooflineRoutes::new(&cfg).unwrap();
        let service = owned
            .compiler()
            .plan_uniform_all_miss(
                "pe",
                "remote",
                Operation::Read,
                &[TransactionRun {
                    payload_bytes: 64,
                    count: 1,
                    request_header_bytes: 8,
                    response_header_bytes: 8,
                }],
            )
            .unwrap();
        assert_eq!(service.sampled_offsets, 1);
    }

    #[test]
    fn address_mix_modulo_uses_the_bounded_sample_window() {
        let mut cfg = config();
        for fabric in cfg.fabrics.as_mut().unwrap() {
            fabric.config.paths.as_mut().unwrap().selection =
                FabricPathSelection::AddressMixModulo { address_shift: 6 };
        }
        let owned = OwnedRooflineRoutes::new(&cfg).unwrap();
        let service = owned
            .compiler()
            .plan_uniform_all_miss(
                "pe",
                "remote",
                Operation::Read,
                &[TransactionRun {
                    payload_bytes: 64,
                    count: 1,
                    request_header_bytes: 8,
                    response_header_bytes: 8,
                }],
            )
            .unwrap();
        assert_eq!(service.sampled_offsets, 4096);
    }
}
