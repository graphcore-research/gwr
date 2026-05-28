// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use clap::Parser;
use gwr_models::fabric::node::FabricRoutingAlgorithm;
use gwr_platform::builder::{
    DEFAULT_CACHE_LINE_SIZE_BYTES, DEFAULT_FABRIC_PORT_BITS_PER_TICK,
    DEFAULT_FABRIC_PORTS_PER_NODE, DEFAULT_FABRIC_ROUTING, DEFAULT_FABRIC_RX_BUFFER_BYTES,
    DEFAULT_FABRIC_TICKS_OVERHEAD, DEFAULT_FABRIC_TICKS_PER_HOP, DEFAULT_FABRIC_TX_BUFFER_BYTES,
    DEFAULT_HBM_DELAY_TICKS, DEFAULT_HBM_SIZE_BYTES, DEFAULT_PE_ADDS_PER_TICK,
    DEFAULT_PE_COMPARES_PER_TICK, DEFAULT_PE_LSU_ACCESS_BYTES, DEFAULT_PE_MULS_PER_TICK,
    DEFAULT_PE_NUM_ACTIVE_REQUESTS, DEFAULT_PE_OVERHEAD_SIZE_BYTES, DEFAULT_PE_SRAM_BYTES,
};
use gwr_platform::types::{
    CacheConfigSection, CacheSection, CoherencyManagerConfigSection, CoherencyManagerSection,
    ConnectSection, FabricKind, FabricPortDevicesSection, FabricPortLocation, FabricSection,
    MemoryDeviceSection, MemoryKind, MemoryMapSection, MemorySection, PlatformConfig,
    ProcessingElementConfigSection, ProcessingElementSection,
};
use gwr_platform::yaml::platform_to_yaml_str;

const FABRIC_NAME: &str = "fabric0";
const PE_MEMORY_MAP_NAME: &str = "default_memory_map";
const COHERENCY_MEMORY_MAP_NAME_PREFIX: &str = "coherency_memory_map";
const COHERENCY_MANAGER_NAME_PREFIX: &str = "cm";

#[derive(Debug, Parser)]
#[command(about = "Build a platform file based around a 2D fabric")]
struct Args {
    #[arg(long, default_value = "platform.yaml")]
    out: PathBuf,

    #[arg(long, default_value_t = DEFAULT_PE_NUM_ACTIVE_REQUESTS)]
    pe_active_requests: usize,

    #[arg(long, default_value_t = DEFAULT_PE_LSU_ACCESS_BYTES)]
    pe_lsu_access_bytes: usize,

    #[arg(long, default_value_t = DEFAULT_PE_SRAM_BYTES)]
    pe_sram_bytes: u64,

    #[arg(long, default_value_t = DEFAULT_PE_ADDS_PER_TICK)]
    pe_adds_per_tick: f64,

    #[arg(long, default_value_t = DEFAULT_PE_MULS_PER_TICK)]
    pe_muls_per_tick: f64,

    #[arg(long, default_value_t = DEFAULT_PE_COMPARES_PER_TICK)]
    pe_compares_per_tick: f64,

    #[arg(long, default_value_t = DEFAULT_PE_OVERHEAD_SIZE_BYTES)]
    pe_overhead_size_bytes: usize,

    #[arg(long, value_enum, default_value_t = FabricKind::Functional)]
    fabric_model: FabricKind,

    #[arg(long, value_enum, default_value_t = DEFAULT_FABRIC_ROUTING)]
    fabric_routing: FabricRoutingAlgorithm,

    #[arg(long, default_value_t = 2)]
    num_columns: usize,

    #[arg(long, default_value_t = 2)]
    num_rows: usize,

    #[arg(long, default_value_t = 2)]
    num_hbms: usize,

    #[arg(long, default_value_t = DEFAULT_HBM_SIZE_BYTES)]
    hbm_base: usize,

    #[arg(long, default_value_t = DEFAULT_HBM_SIZE_BYTES)]
    hbm_size: usize,

    #[arg(long, default_value_t = 0)]
    l1_kib: usize,

    #[arg(long, default_value_t = 32)]
    l1_bytes_per_cycle: usize,

    #[arg(long, default_value_t = 4)]
    l1_num_ways: usize,

    #[arg(long, default_value_t = 5)]
    l1_latency: usize,

    #[arg(long, default_value_t = 0)]
    l2_kib: usize,

    #[arg(long, default_value_t = 32)]
    l2_bytes_per_cycle: usize,

    #[arg(long, default_value_t = 8)]
    l2_num_ways: usize,

    #[arg(long, default_value_t = 20)]
    l2_latency: usize,

    #[arg(long, default_value_t = false)]
    use_coherency_managers: bool,
}

impl Args {
    fn validate(&self) -> Result<(), String> {
        if self.l2_kib != 0 && self.l1_kib == 0 {
            return Err("Cannot have an L2 cache without an L1 cache".to_string());
        }

        let num_nodes = self.num_columns * self.num_rows;
        if self.num_hbms > num_nodes {
            return Err(format!(
                "num-hbms ({}) cannot exceed number of fabric nodes ({num_nodes})",
                self.num_hbms
            ));
        }

        if self.use_coherency_managers && self.l1_kib == 0 && self.l2_kib == 0 {
            return Err("Cannot enable coherency managers without an L1 or L2 cache".to_string());
        }

        Ok(())
    }
}

struct PeIdGen {
    num_rows: usize,
    pe_id: usize,
    max_pe_id: usize,
}

impl PeIdGen {
    fn new(args: &Args) -> Result<Self, String> {
        args.validate()?;
        let num_nodes = args.num_columns * args.num_rows;
        Ok(Self {
            num_rows: args.num_rows,
            pe_id: 0,
            max_pe_id: num_nodes - args.num_hbms,
        })
    }
}

impl Iterator for PeIdGen {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<Self::Item> {
        if self.pe_id < self.max_pe_id {
            let pe_id = self.pe_id;
            self.pe_id += 1;
            Some((pe_id / self.num_rows, pe_id % self.num_rows))
        } else {
            None
        }
    }
}

struct HbmIdGen {
    num_rows: usize,
    mem_id: usize,
    max_mem_id: usize,
}

impl HbmIdGen {
    fn new(args: &Args) -> Result<Self, String> {
        args.validate()?;
        let num_nodes = args.num_columns * args.num_rows;
        Ok(Self {
            num_rows: args.num_rows,
            mem_id: num_nodes - args.num_hbms,
            max_mem_id: num_nodes,
        })
    }
}

impl Iterator for HbmIdGen {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<Self::Item> {
        if self.mem_id < self.max_mem_id {
            let mem_id = self.mem_id;
            self.mem_id += 1;
            Some((mem_id / self.num_rows, mem_id % self.num_rows))
        } else {
            None
        }
    }
}

fn create_name(prefix: &str, column: usize, row: usize) -> String {
    format!("{prefix}_{column}_{row}")
}

fn build_fabrics(args: &Args) -> Vec<FabricSection> {
    vec![FabricSection {
        name: FABRIC_NAME.to_string(),
        kind: args.fabric_model,
        columns: args.num_columns,
        rows: args.num_rows,
        fabric_ports_per_node: Some(if args.use_coherency_managers {
            2
        } else {
            DEFAULT_FABRIC_PORTS_PER_NODE
        }),
        port_devices: Some(
            build_fabric_port_devices(args).expect("validated args should build fabric port map"),
        ),
        ticks_per_hop: Some(DEFAULT_FABRIC_TICKS_PER_HOP),
        ticks_overhead: Some(DEFAULT_FABRIC_TICKS_OVERHEAD),
        rx_buffer_bytes: Some(DEFAULT_FABRIC_RX_BUFFER_BYTES),
        tx_buffer_bytes: Some(DEFAULT_FABRIC_TX_BUFFER_BYTES),
        port_bits_per_tick: Some(DEFAULT_FABRIC_PORT_BITS_PER_TICK),
        routing: Some(args.fabric_routing),
    }]
}

fn build_fabric_port_devices(args: &Args) -> Result<Vec<FabricPortDevicesSection>, String> {
    let mut port_devices = Vec::new();

    for (column, row) in PeIdGen::new(args)? {
        let mut devices = vec![create_name("pe", column, row)];
        if args.l1_kib != 0 {
            devices.push(create_name("l1", column, row));
        }
        if args.l2_kib != 0 {
            devices.push(create_name("l2", column, row));
        }
        port_devices.push(FabricPortDevicesSection {
            port: FabricPortLocation {
                column,
                row,
                port: 0,
            },
            devices,
        });
    }

    for (i, (column, row)) in HbmIdGen::new(args)?.enumerate() {
        port_devices.push(FabricPortDevicesSection {
            port: FabricPortLocation {
                column,
                row,
                port: 0,
            },
            devices: vec![format!("hbm{i}")],
        });
        if args.use_coherency_managers {
            port_devices.push(FabricPortDevicesSection {
                port: FabricPortLocation {
                    column,
                    row,
                    port: 1,
                },
                devices: vec![format!("{COHERENCY_MANAGER_NAME_PREFIX}{i}")],
            });
        }
    }

    Ok(port_devices)
}

fn build_cache(
    name: String,
    kib: usize,
    bytes_per_cycle: usize,
    num_ways: usize,
    latency: usize,
    coherency_manager: Option<String>,
    coherency_managers: Option<Vec<String>>,
) -> CacheSection {
    let num_sets = (kib * 1024) / num_ways / DEFAULT_CACHE_LINE_SIZE_BYTES;
    CacheSection {
        name,
        memory_map: PE_MEMORY_MAP_NAME.to_string(),
        coherency_manager,
        coherency_managers,
        config: CacheConfigSection {
            bw_bytes_per_cycle: Some(bytes_per_cycle),
            line_size_bytes: Some(DEFAULT_CACHE_LINE_SIZE_BYTES),
            num_ways: Some(num_ways),
            num_sets: Some(num_sets),
            delay_ticks: Some(latency),
        },
    }
}

fn build_caches(args: &Args) -> Result<Option<Vec<CacheSection>>, String> {
    if args.l1_kib == 0 && args.l2_kib == 0 {
        return Ok(None);
    }

    let mut caches = Vec::new();
    let coherency_manager_names = if args.use_coherency_managers {
        Some(
            (0..args.num_hbms)
                .map(|i| format!("{COHERENCY_MANAGER_NAME_PREFIX}{i}"))
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };

    for (column, row) in PeIdGen::new(args)? {
        if args.l1_kib != 0 {
            let (coherency_manager, coherency_managers) = if args.l2_kib == 0 {
                if let Some(names) = &coherency_manager_names {
                    if names.len() == 1 {
                        (Some(names[0].clone()), None)
                    } else {
                        (None, Some(names.clone()))
                    }
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };
            caches.push(build_cache(
                create_name("l1", column, row),
                args.l1_kib,
                args.l1_bytes_per_cycle,
                args.l1_num_ways,
                args.l1_latency,
                coherency_manager,
                coherency_managers,
            ));
        }

        if args.l2_kib != 0 {
            let (coherency_manager, coherency_managers) =
                if let Some(names) = &coherency_manager_names {
                    if names.len() == 1 {
                        (Some(names[0].clone()), None)
                    } else {
                        (None, Some(names.clone()))
                    }
                } else {
                    (None, None)
                };
            caches.push(build_cache(
                create_name("l2", column, row),
                args.l2_kib,
                args.l2_bytes_per_cycle,
                args.l2_num_ways,
                args.l2_latency,
                coherency_manager,
                coherency_managers,
            ));
        }
    }

    Ok(Some(caches))
}

fn build_memories(args: &Args) -> Vec<MemorySection> {
    let mut base = args.hbm_base;

    (0..args.num_hbms)
        .map(|i| {
            let mem = MemorySection {
                name: format!("hbm{i}"),
                kind: MemoryKind::HBM,
                base_address: base as u64,
                capacity_bytes: args.hbm_size as u64,
                bw_bytes_per_cycle: None,
                delay_ticks: Some(DEFAULT_HBM_DELAY_TICKS),
            };
            base += args.hbm_size;
            mem
        })
        .collect()
}

fn build_connections(args: &Args) -> Result<Vec<ConnectSection>, String> {
    let mut connections = Vec::new();
    let mut occupied_ports = HashSet::new();

    for (column, row) in PeIdGen::new(args)? {
        let mut entities = vec![format!("pe.{}", create_name("pe", column, row))];

        if args.l1_kib != 0 {
            entities.push(format!("cache.{}", create_name("l1", column, row)));
        }
        if args.l2_kib != 0 {
            entities.push(format!("cache.{}", create_name("l2", column, row)));
        }

        entities.push(format!("fabric.{FABRIC_NAME}@({column},{row})"));
        occupied_ports.insert((column, row, 0usize));

        for pair in entities.windows(2) {
            connections.push(ConnectSection {
                connect: vec![pair[0].clone(), pair[1].clone()],
            });
        }
    }

    for (i, (column, row)) in HbmIdGen::new(args)?.enumerate() {
        occupied_ports.insert((column, row, 0usize));
        connections.push(ConnectSection {
            connect: vec![
                format!("mem.hbm{i}"),
                format!("fabric.{FABRIC_NAME}@({column},{row})"),
            ],
        });
        if args.use_coherency_managers {
            occupied_ports.insert((column, row, 1usize));
            connections.push(ConnectSection {
                connect: vec![
                    format!("coherency_manager.{COHERENCY_MANAGER_NAME_PREFIX}{i}"),
                    format!("fabric.{FABRIC_NAME}@({column},{row}).1"),
                ],
            });
        }
    }

    let fabric_ports_per_node = if args.use_coherency_managers { 2 } else { 1 };
    for column in 0..args.num_columns {
        for row in 0..args.num_rows {
            for port in 0..fabric_ports_per_node {
                if occupied_ports.contains(&(column, row, port)) {
                    continue;
                }
                let fabric_endpoint = if port == 0 {
                    format!("fabric.{FABRIC_NAME}@({column},{row})")
                } else {
                    format!("fabric.{FABRIC_NAME}@({column},{row}).{port}")
                };
                connections.push(ConnectSection {
                    connect: vec!["null".to_string(), fabric_endpoint],
                });
            }
        }
    }

    Ok(connections)
}

fn build_pe_config(args: &Args) -> ProcessingElementConfigSection {
    ProcessingElementConfigSection {
        num_active_requests: Some(args.pe_active_requests),
        lsu_access_bytes: Some(args.pe_lsu_access_bytes),
        overhead_size_bytes: Some(args.pe_overhead_size_bytes),
        sram_bytes: Some(args.pe_sram_bytes),
        adds_per_tick: Some(args.pe_adds_per_tick),
        muls_per_tick: Some(args.pe_muls_per_tick),
        compares_per_tick: Some(args.pe_compares_per_tick),
    }
}

fn build_memory_map_ranges(args: &Args) -> Vec<MemoryDeviceSection> {
    (0..args.num_hbms)
        .map(|mm| MemoryDeviceSection {
            name: format!("hbm{mm}"),
        })
        .collect()
}

fn build_coherency_memory_maps(args: &Args) -> Vec<MemoryMapSection> {
    (0..args.num_hbms)
        .map(|i| MemoryMapSection {
            name: format!("{COHERENCY_MEMORY_MAP_NAME_PREFIX}{i}"),
            devices: vec![MemoryDeviceSection {
                name: format!("hbm{i}"),
            }],
        })
        .collect()
}

fn build_coherency_managers(args: &Args) -> Option<Vec<CoherencyManagerSection>> {
    if !args.use_coherency_managers {
        return None;
    }

    Some(
        (0..args.num_hbms)
            .map(|i| CoherencyManagerSection {
                name: format!("{COHERENCY_MANAGER_NAME_PREFIX}{i}"),
                memory_map: format!("{COHERENCY_MEMORY_MAP_NAME_PREFIX}{i}"),
                config: CoherencyManagerConfigSection {
                    line_size_bytes: Some(DEFAULT_CACHE_LINE_SIZE_BYTES),
                },
            })
            .collect(),
    )
}

fn build_processing_elements(
    args: &Args,
    pe_config: &ProcessingElementConfigSection,
) -> Result<Vec<ProcessingElementSection>, String> {
    Ok(PeIdGen::new(args)?
        .map(|(column, row)| ProcessingElementSection {
            name: create_name("pe", column, row),
            memory_map: PE_MEMORY_MAP_NAME.to_string(),
            config: pe_config.clone(),
        })
        .collect())
}

fn build_platform(args: &Args) -> Result<PlatformConfig, String> {
    let mut memory_maps = vec![MemoryMapSection {
        name: PE_MEMORY_MAP_NAME.to_string(),
        devices: build_memory_map_ranges(args),
    }];
    if args.use_coherency_managers {
        memory_maps.extend(build_coherency_memory_maps(args));
    }
    let pe_config = build_pe_config(args);

    Ok(PlatformConfig {
        memory_maps,
        defaults: None,
        processing_elements: Some(build_processing_elements(args, &pe_config)?),
        caches: build_caches(args)?,
        coherency_managers: build_coherency_managers(args),
        fabrics: Some(build_fabrics(args)),
        memories: Some(build_memories(args)),
        connections: Some(build_connections(args)?),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let platform = build_platform(&args)?;
    let yaml = platform_to_yaml_str(&platform)?;
    fs::write(&args.out, yaml)?;

    Ok(())
}
