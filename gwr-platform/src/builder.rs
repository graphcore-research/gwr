// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, HashMap};

use gwr_engine::sim_error;
use gwr_engine::types::SimError;
use gwr_models::cache::CacheConfig;
use gwr_models::fabric::node::FabricRoutingAlgorithm;
use gwr_models::fabric::{FabricConfig, FabricGeometry, FabricPortConfig, FabricPortSelection};
use gwr_models::memory::MemoryConfig;
use gwr_models::processing_element::ProcessingElementConfig;
use gwr_models::processing_element::compute::OperationClass;

use crate::types::{
    CacheConfigSection, CacheSection, FabricConfigSection, FabricKind, FabricSection,
    MemoryConfigSection, MemorySection, ProcessingElementConfigSection, ProcessingElementSection,
};

pub const DEFAULT_PE_NUM_ACTIVE_REQUESTS: usize = 8;
pub const DEFAULT_PE_LSU_ACCESS_BYTES: usize = 32;
/// Default processing-element scratchpad capacity in bytes.
pub const DEFAULT_PE_SRAM_BYTES: u64 = 1024 * 1024;
pub const DEFAULT_PE_OVERHEAD_SIZE_BYTES: usize = 8;

impl ProcessingElementSection {
    /// Return the model configuration after applying platform defaults.
    ///
    /// # Errors
    ///
    /// Returns an error if sizes or compute capabilities are invalid.
    pub fn effective_config(
        &self,
        operation_classes: &BTreeMap<String, OperationClass>,
    ) -> Result<ProcessingElementConfig, SimError> {
        self.config
            .model_config(operation_classes)
            .map_err(|error| SimError(format!("Processing element '{}': {error}", self.name)))
    }
}

impl ProcessingElementConfigSection {
    pub(crate) fn model_config(
        &self,
        operation_classes: &BTreeMap<String, OperationClass>,
    ) -> Result<ProcessingElementConfig, SimError> {
        let num_active_requests = self
            .num_active_requests
            .unwrap_or(DEFAULT_PE_NUM_ACTIVE_REQUESTS);
        let sram_bytes = usize::try_from(self.sram_bytes.unwrap_or(DEFAULT_PE_SRAM_BYTES))
            .map_err(|error| {
                SimError(format!("SRAM size cannot be represented as usize: {error}"))
            })?;
        let config = ProcessingElementConfig {
            num_active_requests,
            lsu_access_bytes: self.lsu_access_bytes.unwrap_or(DEFAULT_PE_LSU_ACCESS_BYTES),
            overhead_size_bytes: self
                .overhead_size_bytes
                .unwrap_or(DEFAULT_PE_OVERHEAD_SIZE_BYTES),
            sram_bytes,
            compute: self
                .compute
                .clone()
                .ok_or_else(|| SimError("PE execution units must be declared".into()))?,
            operation_classes: operation_classes.clone(),
        };
        config.validate()?;
        Ok(config)
    }
}

impl CacheSection {
    pub(crate) fn effective_config(&self) -> Result<CacheConfig, SimError> {
        self.config
            .model_config()
            .map_err(|error| SimError(format!("Cache '{}': {error}", self.name)))
    }
}

impl CacheConfigSection {
    pub(crate) fn model_config(&self) -> Result<CacheConfig, SimError> {
        let config = CacheConfig::new(
            self.line_size_bytes
                .unwrap_or(DEFAULT_CACHE_LINE_SIZE_BYTES),
            self.bw_bytes_per_tick
                .unwrap_or(DEFAULT_CACHE_BW_BYTES_PER_TICK),
            self.num_sets.unwrap_or(DEFAULT_CACHE_NUM_SETS),
            self.num_ways.unwrap_or(DEFAULT_CACHE_NUM_WAYS),
            self.delay_ticks.unwrap_or(DEFAULT_CACHE_LATENCY_TICKS),
        )
        .with_admission_limits(
            self.max_inflight_parents
                .unwrap_or(DEFAULT_CACHE_MAX_INFLIGHT_PARENTS),
            self.max_inflight_fills
                .unwrap_or(DEFAULT_CACHE_MAX_INFLIGHT_FILLS),
        );
        config.validate()?;
        Ok(config)
    }
}

pub const DEFAULT_CACHE_LINE_SIZE_BYTES: usize = 32;
pub const DEFAULT_CACHE_BW_BYTES_PER_TICK: usize = 32;
pub const DEFAULT_CACHE_NUM_WAYS: usize = 4;
pub const DEFAULT_CACHE_NUM_SETS: usize = 128;
pub const DEFAULT_CACHE_LATENCY_TICKS: usize = 20;
/// Default maximum number of tracked original read requests.
pub const DEFAULT_CACHE_MAX_INFLIGHT_PARENTS: usize = 64;
/// Default maximum number of downstream fills awaiting responses.
pub const DEFAULT_CACHE_MAX_INFLIGHT_FILLS: usize = 64;

pub const DEFAULT_FABRIC_PORTS_PER_NODE: usize = 1;
pub const DEFAULT_FABRIC_TICKS_PER_HOP: usize = 2;
pub const DEFAULT_FABRIC_TICKS_OVERHEAD: usize = 10;
pub const DEFAULT_FABRIC_RX_BUFFER_BYTES: usize = 256;
pub const DEFAULT_FABRIC_TX_BUFFER_BYTES: usize = 256;
pub const DEFAULT_FABRIC_PORT_BITS_PER_TICK: usize = 32 * 8; // 32 bytes per tick

pub const DEFAULT_FABRIC_ROUTING: FabricRoutingAlgorithm = FabricRoutingAlgorithm::ColumnFirst;
pub const DEFAULT_FABRIC_PORT_SELECTION: FabricPortSelection =
    FabricPortSelection::DestinationAddressHash;

impl MemorySection {
    pub(crate) fn effective_config(&self) -> Result<MemoryConfig, SimError> {
        self.config
            .model_config(self.base_address)
            .map_err(|error| SimError(format!("Memory '{}': {error}", self.name)))
    }
}

impl MemoryConfigSection {
    pub(crate) fn model_config(&self, base_address: u64) -> Result<MemoryConfig, SimError> {
        let capacity_bytes = usize::try_from(self.capacity_bytes).map_err(|error| {
            SimError(format!("capacity cannot be represented as usize: {error}"))
        })?;
        let config = MemoryConfig::new(
            base_address,
            capacity_bytes,
            self.bw_bytes_per_tick
                .unwrap_or(DEFAULT_HBM_BW_BYTES_PER_TICK),
            self.delay_ticks.unwrap_or(DEFAULT_HBM_DELAY_TICKS),
        );
        config.validate()?;
        Ok(config)
    }
}

pub const DEFAULT_HBM_DELAY_TICKS: usize = 10;
pub const DEFAULT_HBM_BW_BYTES_PER_TICK: usize = 32;
pub const DEFAULT_HBM_SIZE_BYTES: usize = 1024 * 1024 * 1024;

impl FabricSection {
    pub(crate) fn effective_config(
        &self,
        destination_port_map: FabricDestinationPortMap,
    ) -> Result<(FabricConfig, FabricRoutingAlgorithm), SimError> {
        if self.config.internal_link_bits_per_tick.is_some()
            && matches!(&self.kind, FabricKind::Functional)
        {
            return sim_error!(
                "Fabric '{}': internal link service requires routed fabric",
                self.name
            );
        }
        self.config
            .model_config(self.columns, self.rows, destination_port_map)
            .map_err(|error| SimError(format!("Fabric '{}': {error}", self.name)))
    }
}

impl FabricConfigSection {
    pub(crate) fn model_config(
        &self,
        num_columns: usize,
        num_rows: usize,
        destination_port_map: FabricDestinationPortMap,
    ) -> Result<(FabricConfig, FabricRoutingAlgorithm), SimError> {
        let config = FabricConfig::new(
            FabricGeometry {
                num_columns,
                num_rows,
                num_ports_per_node: self
                    .fabric_ports_per_node
                    .unwrap_or(DEFAULT_FABRIC_PORTS_PER_NODE),
                ports_per_node_limit: None,
            },
            FabricPortConfig {
                ticks_per_hop: self.ticks_per_hop.unwrap_or(DEFAULT_FABRIC_TICKS_PER_HOP),
                ticks_overhead: self.ticks_overhead.unwrap_or(DEFAULT_FABRIC_TICKS_OVERHEAD),
                rx_buffer_bytes: self
                    .rx_buffer_bytes
                    .unwrap_or(DEFAULT_FABRIC_RX_BUFFER_BYTES),
                tx_buffer_bytes: self
                    .tx_buffer_bytes
                    .unwrap_or(DEFAULT_FABRIC_TX_BUFFER_BYTES),
                port_bits_per_tick: self
                    .port_bits_per_tick
                    .unwrap_or(DEFAULT_FABRIC_PORT_BITS_PER_TICK),
            },
            destination_port_map,
        )?
        .with_port_selection(self.port_selection.unwrap_or(DEFAULT_FABRIC_PORT_SELECTION))
        .with_internal_link_bits_per_tick(self.internal_link_bits_per_tick)?;
        Ok((config, self.routing.unwrap_or(DEFAULT_FABRIC_ROUTING)))
    }
}

/// Per-fabric destination device IDs and their eligible egress ports.
pub type FabricDestinationPortMaps = HashMap<String, FabricDestinationPortMap>;

/// Destination device IDs mapped to eligible fabric egress port indices.
pub type FabricDestinationPortMap = HashMap<u64, Vec<usize>>;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use gwr_engine::types::{DeviceId, SimError};
    use yaml_serde::from_str;

    use super::FabricDestinationPortMaps;
    fn build_fabric_destination_port_maps(
        mut cfg: PlatformConfig,
        ids: &DeviceIds,
    ) -> Result<FabricDestinationPortMaps, SimError> {
        for pe in cfg.processing_elements.iter_mut().flatten() {
            pe.config.compute = Some(yaml_serde::from_str("execution_units: { vector: { capabilities: [{ class: add, ops_per_tick: 1 }] } }").unwrap());
        }
        cfg.operation_classes.insert(
            "add".into(),
            yaml_serde::from_str("{ operations: [add], arity: 2 }").unwrap(),
        );
        let resolved = cfg.resolve()?;
        Ok(resolved
            .fabrics
            .iter()
            .map(|fabric| {
                let ports = fabric
                    .config
                    .destination_port_map()
                    .iter()
                    .filter_map(|(id, ports)| {
                        ids.get(&resolved.device_names_by_id[id])
                            .map(|id| (id.0, ports.clone()))
                    })
                    .collect::<HashMap<_, _>>();
                (fabric.name.clone(), ports)
            })
            .collect())
    }
    use crate::DeviceIds;
    use crate::types::PlatformConfig;

    #[test]
    fn resolves_memory_maps_from_effective_memory_ranges() {
        let cfg: PlatformConfig = from_str(
            r"
operation_classes:
  add: { operations: [add], arity: 2 }
memory_maps:
  - { name: mm0, devices: [hbm0] }
processing_elements:
  - name: pe0
    memory_map: mm0
    config:
      compute:
        execution_units:
          vector:
            capabilities: [{ class: add, ops_per_tick: 1 }]
memories:
  - { name: hbm0, kind: hbm, base_address: 0x4000, config: { capacity_bytes: 0x2000 } }
",
        )
        .unwrap();
        let resolved = cfg.resolve().expect("configuration resolves");
        let memory_map = &resolved.pes[0].memory_map;
        assert_eq!(memory_map.num_regions(), 1);
        assert_eq!(memory_map.lookup(0x4000), Some((DeviceId(1), 0)));
        assert_eq!(memory_map.lookup(0x5fff), Some((DeviceId(1), 0x1fff)));
        assert_eq!(memory_map.lookup(0x6000), None);
    }

    #[test]
    fn derives_fabric_port_maps_from_direct_connections() {
        let cfg: PlatformConfig = from_str(
            "
memory_maps:
  - name: mm0
    devices:
      - hbm0
processing_elements:
  - name: pe0
    memory_map: mm0
    config: {}
fabrics:
  - name: fabric0
    kind: functional
    columns: 2
    rows: 1
    config: {}
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config:
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

        let maps =
            build_fabric_destination_port_maps(cfg, &device_ids).expect("fabric maps should build");
        let fabric_map = maps.get("fabric0").expect("fabric map should exist");

        assert_eq!(fabric_map.get(&0), Some(&vec![0]));
        assert_eq!(fabric_map.get(&1), Some(&vec![1]));
    }

    #[test]
    fn derives_fabric_port_maps_through_cache() {
        let cfg: PlatformConfig = from_str(
            "
memory_maps:
  - name: mm0
    devices:
      - hbm0
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
    config: {}
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config:
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

        let maps =
            build_fabric_destination_port_maps(cfg, &device_ids).expect("fabric maps should build");
        let fabric_map = maps.get("fabric0").expect("fabric map should exist");

        assert_eq!(fabric_map.get(&0), Some(&vec![0]));
        assert_eq!(fabric_map.get(&1), Some(&vec![1]));
    }

    #[test]
    fn derives_multiple_candidate_ports_for_same_device() {
        let cfg: PlatformConfig = from_str(
            "
memory_maps:
  - name: mm0
    devices:
      - hbm0
fabrics:
  - name: fabric0
    kind: functional
    columns: 2
    rows: 1
    config:
      fabric_ports_per_node: 2
  - name: fabric1
    kind: functional
    columns: 3
    rows: 1
    config: {}
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config:
      capacity_bytes: 1024
connections:
  - connect:
    - fabric.fabric1@(0,0)
    - fabric.fabric0@(1,0)
  - connect:
    - fabric.fabric1@(1,0)
    - fabric.fabric0@(1,0).1
  - connect: [mem.hbm0, 'fabric.fabric1@(2,0)']
",
        )
        .expect("platform yaml should parse");
        let device_ids = DeviceIds::from([("hbm0".to_string(), DeviceId(7))]);

        let maps =
            build_fabric_destination_port_maps(cfg, &device_ids).expect("fabric maps should build");
        let fabric_map = maps.get("fabric0").expect("fabric map should exist");

        assert_eq!(fabric_map.get(&7), Some(&vec![2, 3]));
    }

    #[test]
    fn derives_fabric_port_maps_through_single_fabric_link() {
        let cfg: PlatformConfig = from_str(
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
    config: {}
  - name: fabric1
    kind: functional
    columns: 2
    rows: 1
    config: {}
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

        let maps =
            build_fabric_destination_port_maps(cfg, &device_ids).expect("fabric maps should build");
        let fabric0_map = maps.get("fabric0").expect("fabric0 map should exist");
        let fabric1_map = maps.get("fabric1").expect("fabric1 map should exist");

        assert_eq!(fabric0_map.get(&0), Some(&vec![0]));
        assert_eq!(fabric0_map.get(&1), Some(&vec![1]));
        assert_eq!(fabric1_map.get(&0), Some(&vec![1]));
        assert_eq!(fabric1_map.get(&1), Some(&vec![0]));
    }

    #[test]
    fn derives_fabric_port_maps_through_two_fabric_links() {
        let cfg: PlatformConfig = from_str(
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
    config: {}
  - name: fabric1
    kind: functional
    columns: 3
    rows: 1
    config: {}
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

        let maps =
            build_fabric_destination_port_maps(cfg, &device_ids).expect("fabric maps should build");
        let fabric0_map = maps.get("fabric0").expect("fabric0 map should exist");
        let fabric1_map = maps.get("fabric1").expect("fabric1 map should exist");

        assert_eq!(fabric0_map.get(&0), Some(&vec![0]));
        assert_eq!(fabric0_map.get(&1), Some(&vec![1, 2]));
        assert_eq!(fabric1_map.get(&0), Some(&vec![0, 1]));
        assert_eq!(fabric1_map.get(&1), Some(&vec![2]));
    }

    #[test]
    fn derives_only_shortest_fabric_port_maps_through_cycle() {
        let cfg: PlatformConfig = from_str(
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
    config: {}
  - name: fabric1
    kind: functional
    columns: 3
    rows: 1
    config: {}
  - name: fabric2
    kind: functional
    columns: 3
    rows: 1
    config: {}
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

        let maps =
            build_fabric_destination_port_maps(cfg, &device_ids).expect("fabric maps should build");
        let fabric0_map = maps.get("fabric0").expect("fabric0 map should exist");
        let fabric1_map = maps.get("fabric1").expect("fabric1 map should exist");
        let fabric2_map = maps.get("fabric2").expect("fabric2 map should exist");

        assert_eq!(fabric0_map.get(&1), Some(&vec![1]));
        assert_eq!(fabric1_map.get(&1), Some(&vec![0]));
        assert_eq!(fabric2_map.get(&1), Some(&vec![1]));
    }
}
