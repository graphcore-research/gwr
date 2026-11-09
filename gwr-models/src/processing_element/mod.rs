// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A Processing Element (PE) for a simulation.
//!
//! The PE performs computations defined by a timetable.
//!
//! The PE comprises:
//!  - Load/Store
//!  - Internal Buffers
//!  - Compute
//!
//! Identifies all compute nodes that can execute
//! because their dependencies are satisfied (or they have no dependencies).

//! # Ports
//!
//! Each PE has:
//!  - One [input port](gwr_engine::port::InPort): `rx`
//!  - One [output port](gwr_engine::port::OutPort): `tx`
//!
//! that are managed by the `LoadStoreUnit`

use std::cell::RefCell;
use std::fmt::{self, Display};
use std::rc::Rc;

use async_trait::async_trait;
use compute::{ComputeConfig, ComputeModel, ComputeScheduler};
use gwr_components::capacity_allocator::CapacityAllocator;
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::port::PortStateResult;
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::Runnable;
use gwr_engine::types::{DeviceId, SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::debug;
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;
use serde::{Deserialize, Serialize};

use crate::log_stats;
use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::MemoryMap;
use crate::processing_element::dispatch::Dispatch;
use crate::processing_element::flop_monitor::FlopMonitor;
use crate::processing_element::load_store_unit::LoadStoreUnit;

mod activity;
use activity::{ActivityLanes, ProcessingElementActivityLanes};
pub mod block_execution;
mod block_runtime;
pub use block_runtime::BlockExecutionSnapshot;
pub use load_store_unit::LsuIssueSnapshot;
mod boundary_payload;
pub mod compute;
pub mod dispatch;
mod flop_monitor;
mod load_store_unit;
pub mod local_payload;
mod packet_arbiter;
pub mod program;
pub(crate) mod runner;
use runner::TaskRunner;
pub use runner::estimate_peak_tile_sram;
pub(crate) use runner::streamed_matrix::LocalOperandHandle;
pub mod model;
pub mod task;
pub mod tensors;

#[derive(EntityGet, EntityDisplay)]
pub struct ProcessingElement {
    entity: Rc<Entity>,
    atomic_memory_map: Rc<MemoryMap>,
    lsu: Rc<LoadStoreUnit>,
    dma: RefCell<Option<Rc<LoadStoreUnit>>>,
    transfer_resource: gwr_resources::Resource,
    clock: Clock,
    spawner: Spawner,

    compute_model: Rc<ComputeModel>,
    compute_scheduler: ComputeScheduler,
    sram: CapacityAllocator,
    sram_bytes: usize,
    block_execution: Option<Rc<block_runtime::BlockExecutionResources>>,
    boundary_payload: Option<Rc<boundary_payload::BoundaryPayloadService>>,
    local_payload: Option<Rc<boundary_payload::BoundaryPayloadService>>,
    stats: Rc<RefCell<ProcessingElementStats>>,
    activity_lanes: Rc<ProcessingElementActivityLanes>,
    dispatcher: RefCell<Option<Dispatcher>>,
    flop_monitor: Option<Rc<FlopMonitor>>,
}

impl ProcessingElement {
    /// Static admitted target/path/group validation, before activation. Does
    /// not ask the live service whether a future control milestone happened.
    pub(crate) fn validate_streamed_target(
        &self,
        config: &runner::streamed_matrix::StreamedMatrixTaskConfig,
        target: &crate::memory::atomic::AtomicTargetSender,
    ) -> SimResult {
        // Validate only the source-selected valid rows/column-block words;
        // unrelated unused C columns need not map. All four bytes of every
        // selected BF16 pair must resolve to the same actual shared target.
        for row in 0..config.m {
            for col in (0..64).step_by(2) {
                let address = config.c.tensor().addr()
                    + ((row * config.n + config.n_offset + col) * 2) as u64;
                self.validate_atomic_memory_binding(target, target.device(), address)?;
            }
        }
        for chunk in 0..config.chunk_count() {
            self.lsu
                .validate_source_target(config.issues(chunk)?, target.device())?;
        }
        Ok(())
    }
    pub fn preflight_streamed_matrix(
        &self,
        config: &runner::streamed_matrix::StreamedMatrixTaskConfig,
    ) -> SimResult {
        config.validate()?;
        if self.sram_bytes < runner::streamed_matrix::LDS_BYTES {
            return Err(gwr_engine::types::SimError(
                "stream LDS requires 49152B".into(),
            ));
        }
        for chunk in 0..config.chunk_count() {
            self.lsu.validate_source_issues(config.issues(chunk)?)?;
        }
        let work = compute::ComputeWork::matrix_mac(
            vec![
                tensors::dtype::DataType::Bf16,
                tensors::dtype::DataType::Bf16,
            ],
            compute::MatrixWorkShape {
                m: 32,
                n: 64,
                k: 128,
            },
        )?
        .with_execution(Some(config.execution.clone()));
        self.compute_model.lower(&work)?;
        self.compute_model.lower(&compute::ComputeWork::operation(
            "convert",
            vec![tensors::dtype::DataType::Fp32],
            2048,
        ))?;
        Ok(())
    }
    pub fn preflight_atomic_task(&self, config: &task::AtomicTaskConfig) -> SimResult {
        config.validate_for_pe(self)
    }
    pub fn validate_atomic_memory_binding(
        &self,
        target: &crate::memory::atomic::AtomicTargetSender,
        device: u64,
        address: u64,
    ) -> SimResult {
        self.validate_atomic_word_mapping(device, address)?;
        self.validate_atomic_direct_target(target)
    }
    pub(crate) fn validate_atomic_direct_target(
        &self,
        target: &crate::memory::atomic::AtomicTargetSender,
    ) -> SimResult {
        self.lsu.validate_atomic_direct_target(target)
    }
    pub fn validate_atomic_word_mapping(&self, device: u64, address: u64) -> SimResult {
        for offset in 0..4 {
            let byte = address
                .checked_add(offset)
                .ok_or_else(|| SimError("atomic word address overflows".into()))?;
            if self.atomic_memory_map.is_striped_address(byte) {
                return Err(SimError(
                    "direct atomic address translation in striped memory is unsupported".into(),
                ));
            }
            if self.atomic_memory_map.lookup(byte).map(|(id, _)| id.0) != Some(device) {
                return Err(SimError(
                    "assigned PE atomic word is unmapped or maps to another memory".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        memory_map: &Rc<MemoryMap>,
        pe_config: &ProcessingElementConfig,
        device_id: DeviceId,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(
            engine, clock, parent, name, None, memory_map, pe_config, device_id,
        )
    }

    #[expect(clippy::too_many_arguments)]
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        memory_map: &Rc<MemoryMap>,
        pe_config: &ProcessingElementConfig,
        device_id: DeviceId,
    ) -> Result<Rc<Self>, SimError> {
        pe_config
            .validate()
            .map_err(|error| SimError(format!("{parent}::{name}: {error}")))?;
        let local_payload = pe_config
            .local_payload_service
            .map(|rate| boundary_payload::BoundaryPayloadService::new(rate, clock))
            .transpose()?;
        let boundary_payload = pe_config
            .boundary_payload_service
            .map(|rate| boundary_payload::BoundaryPayloadService::new(rate, clock))
            .transpose()?;
        let entity = Rc::new(Entity::new(parent, name));

        let lsu = LoadStoreUnit::new_and_register(
            engine, clock, &entity, aka, pe_config, memory_map, device_id,
        )?;
        let monitor_window_size = entity.tracker.monitoring_window_size_for(entity.id);
        let flop_monitor = monitor_window_size.map(|window_size_ticks| {
            FlopMonitor::new_and_register(engine, &entity, clock, window_size_ticks)
        });

        let compute_model = Rc::new(ComputeModel::new(
            &pe_config.compute,
            &pe_config.operation_classes,
        )?);
        let compute_scheduler = ComputeScheduler::new(&compute_model);
        let sram = CapacityAllocator::new(&entity, "sram", pe_config.sram_bytes, "bytes")?;
        let activity_lanes = Rc::new(ProcessingElementActivityLanes::new(
            entity.clone(),
            &compute_model,
        ));
        let block_execution = pe_config
            .block_execution
            .clone()
            .map(|config| block_runtime::BlockExecutionResources::new(config, clock, &entity))
            .transpose()?;
        let rc_self = Rc::new(Self {
            entity: entity.clone(),
            atomic_memory_map: memory_map.clone(),
            lsu,
            dma: RefCell::new(None),
            transfer_resource: gwr_resources::Resource::new(1),
            clock: clock.clone(),
            spawner: engine.spawner(),

            compute_model,
            compute_scheduler,
            sram,
            sram_bytes: pe_config.sram_bytes,
            block_execution,
            boundary_payload,
            local_payload,
            stats: Rc::new(RefCell::new(ProcessingElementStats::default())),
            activity_lanes,

            dispatcher: RefCell::new(None),
            flop_monitor,
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    /// Validate a copy's views and memory mappings before simulation starts.
    pub fn preflight_transfer_task(
        &self,
        config: &crate::transfer::TransferTaskConfig,
    ) -> SimResult {
        config.validate()?;
        let issuer = self
            .dma
            .borrow()
            .clone()
            .unwrap_or_else(|| self.lsu.clone());
        issuer.validate_mapped_view(&config.source)?;
        issuer.validate_mapped_view(&config.destination)?;
        Ok(())
    }

    /// Add an independent DMA memory issuer before simulation starts.
    pub fn configure_dma(
        &self,
        engine: &Engine,
        clock: &Clock,
        memory_map: &Rc<MemoryMap>,
        config: &ProcessingElementConfig,
        device_id: DeviceId,
    ) -> SimResult {
        let dma_parent = Rc::new(Entity::new(&self.entity, "dma"));
        let lsu = LoadStoreUnit::new_and_register(
            engine,
            clock,
            &dma_parent,
            None,
            config,
            memory_map,
            device_id,
        )?;
        *self.dma.borrow_mut() = Some(lsu);
        Ok(())
    }

    pub fn connect_dma_tx(&self, port: PortStateResult<MemoryAccess>) -> SimResult {
        self.dma
            .borrow()
            .as_ref()
            .ok_or_else(|| SimError("DMA interface is not configured".into()))?
            .connect_port_tx(port)
    }

    pub fn dma_rx(&self) -> PortStateResult<MemoryAccess> {
        match self.dma.borrow().as_ref() {
            Some(dma) => dma.port_rx(),
            None => Err(SimError("DMA interface is not configured".into())),
        }
    }

    pub fn set_dispatcher(&self, dispatcher: &Dispatcher) {
        *self.dispatcher.borrow_mut() = Some(dispatcher.clone());
    }

    pub fn connect_port_tx(&self, port_state: PortStateResult<MemoryAccess>) -> SimResult {
        self.lsu.connect_port_tx(port_state)
    }

    pub fn port_rx(&self) -> PortStateResult<MemoryAccess> {
        self.lsu.port_rx()
    }

    #[must_use]
    pub fn total_graph_nodes(&self) -> usize {
        match self.dispatcher.borrow().as_ref() {
            None => 0,
            Some(dispatcher) => dispatcher.total_tasks_for_pe(self.entity.name.as_str()),
        }
    }

    #[must_use]
    pub fn total_flops(&self) -> usize {
        self.stats.borrow().machine_ops.flop_equivalents()
    }

    /// Enable aggregate compute counters before executing any work.
    pub fn enable_compute_action_counters(&self) -> SimResult {
        let mut stats = self.stats.borrow_mut();
        if !stats.machine_ops.by_operation.is_empty() {
            return sim_error!("compute counters must be enabled before work starts");
        }
        stats.compute_actions.get_or_insert_with(Default::default);
        Ok(())
    }
    /// Declared engine names, including engines with zero completed work.
    pub fn compute_engine_names(&self) -> &[String] {
        self.compute_model.engine_names()
    }

    /// Completed typed compute service counters, available without tracing.
    pub fn compute_action_snapshot(&self) -> Vec<compute::ComputeCounterSnapshot> {
        self.stats
            .borrow()
            .compute_actions
            .as_ref()
            .map(|c| c.snapshot(self.compute_model.engine_names()))
            .unwrap_or_default()
    }

    #[must_use]
    pub fn machine_ops(&self) -> MachineOpCounts {
        self.stats.borrow().machine_ops.clone()
    }

    #[must_use]
    pub fn bytes_consumed(&self) -> usize {
        self.stats.borrow().read_bytes
    }

    /// Logical output bytes produced by completed work.
    #[must_use]
    pub fn bytes_produced(&self) -> usize {
        self.stats.borrow().written_bytes
    }

    #[must_use]
    pub fn memory_traffic_bytes(&self) -> (usize, usize) {
        let stats = self.stats.borrow();
        (stats.read_bytes, stats.written_bytes)
    }

    /// Validate typed operations and peak tile storage without issuing requests
    /// or reserving engines. This uses the same lowering and admission rules as
    /// execution, so unsupported modes fail before publishing a workload.
    pub fn preflight_program_task(&self, config: &task::ProgramTaskConfig) -> SimResult {
        self.lsu.validate_program_views(config)?;
        if config.pipeline.is_some() {
            for view in config.inputs.iter().chain(&config.outputs) {
                self.lsu.validate_aligned_view(view)?;
            }
            if self.boundary_payload.is_some() {
                return sim_error!(
                    "pipeline with boundary_payload_service is unsupported; use block_execution local_memory_service"
                );
            }
            let resources = self
                .block_execution
                .as_ref()
                .ok_or_else(|| SimError("pipeline requires block_execution support".into()))?;
            let plan =
                runner::program_pipeline::compile(config, &self.compute_model, &resources.config)?;
            if self.local_payload.is_some() {
                return sim_error!("pipeline with local_payload_service is unsupported");
            }
            plan.validate_capacity(config, self.sram_bytes)?;
            return Ok(());
        }
        local_payload::preflight(config, self.local_payload.is_some(), self.sram_bytes)?;
        let region = program::validate::validate_program(
            &config.program,
            &program::validate::RegionBindings {
                inputs: config.inputs.clone(),
                outputs: config.outputs.clone(),
            },
        )?;
        let schedule = program::tile::plan_tiles(&region, config.tiling.as_ref())?;
        runner::preflight_program(&self.compute_model, &region, &schedule)?;
        let tiling = config.tiling.clone().unwrap_or_default();
        let peak =
            estimate_peak_tile_sram(&config.program, &config.inputs, &config.outputs, &tiling)?;
        if peak > self.sram_bytes {
            return sim_error!(
                "{} tile requires {peak} SRAM bytes, available {}",
                config.id,
                self.sram_bytes
            );
        }
        Ok(())
    }

    #[must_use]
    /// Local logical service snapshot; never included in external PE/HBM
    /// counters.
    pub fn local_payload_snapshot(&self) -> Option<local_payload::LocalPayloadSnapshot> {
        self.local_payload.as_ref().map(|s| s.snapshot().into())
    }

    /// Aggregate block pipeline service and storage observations.
    pub fn block_execution_snapshot(&self) -> Option<BlockExecutionSnapshot> {
        self.block_execution
            .as_ref()
            .map(|resources| resources.snapshot())
    }

    /// Actual packet counts and producer waits, shared by ordinary and
    /// pipelined transfers.
    #[must_use]
    pub fn lsu_issue_snapshot(&self) -> LsuIssueSnapshot {
        self.lsu.issue_snapshot()
    }

    /// Pure complete workspace demand using this PE's actual compute and stage
    /// configuration.
    pub fn program_workspace_bytes(
        &self,
        config: &task::ProgramTaskConfig,
    ) -> Result<usize, SimError> {
        estimate_program_workspace(
            config,
            &self.compute_model,
            self.block_execution.as_ref().map(|r| &r.config),
        )
    }

    pub fn peak_sram_bytes(&self) -> usize {
        self.stats.borrow().peak_sram_bytes
    }

    pub fn dump_stats(&self, time_now_ns: f64) {
        let stats = self.stats.borrow();
        log_stats(
            &self.entity,
            ProcessingElementStatsDisplay::new(
                format!("ProcessingElement {}", self.entity.full_name()),
                time_now_ns,
                stats.machine_ops.clone(),
            )
            .with_data(stats.read_bytes, stats.written_bytes),
        );
        if let Some(snapshot) = self.block_execution_snapshot()
            && snapshot.peak_reserved_window_bytes != 0
        {
            gwr_track::info!(self.entity; "Block pipeline: {snapshot:?}");
            gwr_track::info!(self.entity; "LSU issue: {:?}", self.lsu_issue_snapshot());
        }
    }
}

#[async_trait(?Send)]
impl Runnable for ProcessingElement {
    #[expect(
        clippy::too_many_lines,
        reason = "Keep task admission and completion ownership in the runnable scope"
    )]
    async fn run(&self) -> SimResult {
        let dispatcher = self
            .dispatcher
            .borrow()
            .as_ref()
            .ok_or_else(|| SimError("Started without dispatcher".to_string()))?
            .clone();

        let pe_name = self.entity.name.as_str();
        let (mut complete, mut ready_node_indices) = dispatcher.ready_task_indices(pe_name)?;

        loop {
            if complete {
                break;
            }
            if ready_node_indices.is_empty() {
                // Wait for something to change
                dispatcher.wait_for_ready(pe_name).await?;
            } else {
                // Spawn all so they can run in parallel
                for task_idx in ready_node_indices.drain(..) {
                    match dispatcher.task_by_id(task_idx)? {
                        task::Task::ProgramTask { config } => {
                            self.preflight_program_task(&config)?;
                        }
                        task::Task::TransferTask { config } => {
                            self.preflight_transfer_task(&config)?;
                        }
                        task::Task::StreamedMatrix { config } => config.validate_for_pe(self)?,
                        task::Task::AtomicEffect { config } => {
                            self.preflight_atomic_task(&config)?;
                        }
                        task::Task::AtomicControlJoin { config } => config.validate_for_pe(self)?,
                        task::Task::SyncTask { .. } => (),
                    }
                    dispatcher.set_task_active(task_idx)?;

                    if let task::Task::TransferTask { config } = dispatcher.task_by_id(task_idx)? {
                        let lsu = self
                            .dma
                            .borrow()
                            .clone()
                            .unwrap_or_else(|| self.lsu.clone());
                        let resource = self.transfer_resource.clone();
                        let lanes = self.activity_lanes.clone();
                        let stats = self.stats.clone();
                        let dispatcher = dispatcher.clone();
                        self.spawner.spawn(async move {
                            config.validate()?;
                            let _reservation =
                                gwr_resources::base::ResourceGuard::new(resource).await;
                            let group = lanes.create_group(&config.id);
                            for (source, destination) in config.chunks() {
                                let bytes = usize::try_from(source.end - source.start).unwrap();
                                lsu.access_ranges(
                                    gwr_engine::types::AccessType::ReadRequest,
                                    [source],
                                    &lanes.lsu_read,
                                    &config.id,
                                    &group,
                                )
                                .await?;
                                lsu.access_ranges(
                                    gwr_engine::types::AccessType::WriteNonPostedRequest,
                                    [destination],
                                    &lanes.lsu_write,
                                    &config.id,
                                    &group,
                                )
                                .await?;
                                let mut stats = stats.borrow_mut();
                                stats.read_bytes = stats
                                    .read_bytes
                                    .checked_add(bytes)
                                    .ok_or_else(|| SimError("transfer reads overflow".into()))?;
                                stats.written_bytes = stats
                                    .written_bytes
                                    .checked_add(bytes)
                                    .ok_or_else(|| SimError("transfer writes overflow".into()))?;
                            }
                            dispatcher.set_task_completed(task_idx)
                        });
                        continue;
                    }
                    let runner = TaskRunner {
                        entity: self.entity.clone(),
                        clock: self.clock.clone(),
                        dispatcher: dispatcher.clone(),
                        lsu: self.lsu.clone(),
                        compute_model: self.compute_model.clone(),
                        compute_scheduler: self.compute_scheduler.clone(),
                        sram: self.sram.clone(),
                        sram_bytes: self.sram_bytes,
                        block_execution: self.block_execution.clone(),
                        boundary_payload: self.boundary_payload.clone(),
                        local_payload: self.local_payload.clone(),
                        stats: self.stats.clone(),
                        activity_lanes: self.activity_lanes.clone(),
                        flop_monitor: self.flop_monitor.clone(),
                    };
                    self.spawner
                        .spawn(async move { Box::pin(runner.run(task_idx)).await });
                }
            }

            (complete, ready_node_indices) = dispatcher.ready_task_indices(pe_name)?;
        }
        debug!(self.entity ; "PE {pe_name} DONE");
        Ok(())
    }
}

pub struct ProcessingElementStatsDisplay {
    prefix: String,
    time_now_ns: f64,
    machine_ops: MachineOpCounts,
    bytes_consumed: usize,
    bytes_produced: usize,
}

impl ProcessingElementStatsDisplay {
    #[must_use]
    pub fn new(prefix: impl Into<String>, time_now_ns: f64, machine_ops: MachineOpCounts) -> Self {
        Self {
            prefix: prefix.into(),
            time_now_ns,
            machine_ops,
            bytes_consumed: 0,
            bytes_produced: 0,
        }
    }

    #[must_use]
    pub fn with_data(mut self, bytes_consumed: usize, bytes_produced: usize) -> Self {
        self.bytes_consumed = bytes_consumed;
        self.bytes_produced = bytes_produced;
        self
    }
}

impl Display for ProcessingElementStatsDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total_flops = self.machine_ops.flop_equivalents as u128;
        let time_now_s = self.time_now_ns / 1e9;
        let total_gflops = total_flops as f64 / 1e9;
        let average_gflops_per_second = if time_now_s == 0.0 {
            0.0
        } else {
            total_gflops / time_now_s
        };

        writeln!(f, "{}:", self.prefix)?;
        writeln!(
            f,
            "  FLOPs: {total_flops}, {total_gflops:.2} GFLOPs, {average_gflops_per_second:.2} GFLOP/s"
        )?;
        writeln!(f, "  Machine ops: {:?}", self.machine_ops.by_operation)?;
        write!(
            f,
            "  Data: {} bytes consumed, {} bytes produced",
            self.bytes_consumed, self.bytes_produced
        )
    }
}

type Dispatcher = Rc<dyn Dispatch>;

#[derive(Clone)]
pub struct ProcessingElementConfig {
    /// The number of outstanding requests can the LSU handle at once
    pub num_active_requests: usize,

    /// The maximum number of bytes in each memory access
    pub lsu_access_bytes: usize,

    /// The number of bytes of protocol overhead for each memory transaction
    pub overhead_size_bytes: usize,

    /// The total number of local SRAM storage bytes available to the PE
    pub sram_bytes: usize,

    /// Optional caller combined-direction packed boundary B/actual1ns tick.
    /// This is not a physical SRAM/VMEM rate; None preserves the old path.
    pub boundary_payload_service: Option<u64>,
    /// Separate combined-direction logical local debt budget; None is legacy.
    pub local_payload_service: Option<u64>,
    /// Optional PE-wide bounded block execution support.
    pub block_execution: Option<block_execution::BlockExecutionConfig>,

    /// Typed compute engines.
    pub compute: ComputeConfig,
    pub operation_classes: std::collections::BTreeMap<String, compute::OperationClass>,
}

impl ProcessingElementConfig {
    pub fn validate(&self) -> SimResult {
        if self.num_active_requests == 0 {
            return sim_error!("LSU request-slot count must be greater than zero");
        }
        if self.lsu_access_bytes == 0 {
            return sim_error!("LSU access size must be greater than zero");
        }
        if self.sram_bytes == 0 {
            return sim_error!("PE SRAM capacity must be greater than zero");
        }
        if let Some(rate) = self.boundary_payload_service {
            boundary_payload::BoundaryPayloadService::validate_rate(rate)?;
        }
        if let Some(rate) = self.local_payload_service {
            boundary_payload::BoundaryPayloadService::validate_rate(rate)?;
        }
        if let Some(config) = &self.block_execution {
            config.validate()?;
            if config.local_memory_service.is_some() && self.local_payload_service.is_some() {
                return sim_error!(
                    "block_execution local_memory_service cannot share a PE with local_payload_service"
                );
            }
        }
        ComputeModel::new(&self.compute, &self.operation_classes)?;
        Ok(())
    }
}

#[derive(Default)]
struct ProcessingElementStats {
    compute_actions: Option<compute::ComputeCounters>,
    machine_ops: MachineOpCounts,
    read_bytes: usize,
    written_bytes: usize,
    peak_sram_bytes: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MachineOpCounts {
    pub by_operation: std::collections::BTreeMap<String, usize>,
    pub flop_equivalents: usize,
}

impl MachineOpCounts {
    pub fn single(op: &str, count: usize, flops_per_op: usize) -> Result<Self, SimError> {
        Ok(Self {
            by_operation: std::collections::BTreeMap::from([(op.to_owned(), count)]),
            flop_equivalents: count
                .checked_mul(flops_per_op)
                .ok_or_else(|| SimError("FLOP-equivalent count overflows".into()))?,
        })
    }

    #[must_use]
    pub fn flop_equivalents(&self) -> usize {
        self.flop_equivalents
    }

    pub fn checked_operation_total(&self) -> Result<usize, SimError> {
        self.by_operation.values().try_fold(0usize, |total, count| {
            total
                .checked_add(*count)
                .ok_or_else(|| SimError("machine operation count overflows".into()))
        })
    }

    pub fn checked_add(self, other: Self) -> Result<Self, SimError> {
        let mut by_operation = self.by_operation;
        for (op, count) in other.by_operation {
            let entry = by_operation.entry(op).or_default();
            *entry = entry
                .checked_add(count)
                .ok_or_else(|| SimError("machine operation count overflows".into()))?;
        }
        let flop_equivalents = self
            .flop_equivalents
            .checked_add(other.flop_equivalents)
            .ok_or_else(|| SimError("FLOP-equivalent count overflows".into()))?;
        Ok(Self {
            by_operation,
            flop_equivalents,
        })
    }
}

/// Pure config-aware workspace estimation for ordinary and pipelined programs.
pub fn estimate_program_workspace(
    config: &task::ProgramTaskConfig,
    model: &ComputeModel,
    limits: Option<&block_execution::BlockExecutionConfig>,
) -> Result<usize, SimError> {
    if config.pipeline.is_some() {
        Ok(runner::program_pipeline::compile(
            config,
            model,
            limits.ok_or_else(|| SimError("pipeline requires block_execution support".into()))?,
        )?
        .bytes())
    } else {
        estimate_peak_tile_sram(
            &config.program,
            &config.inputs,
            &config.outputs,
            &config.tiling.clone().unwrap_or_default(),
        )
    }
}
