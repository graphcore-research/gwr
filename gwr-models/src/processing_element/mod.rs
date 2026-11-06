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
use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::rc::Rc;

use async_trait::async_trait;
use compute::{ComputeConfig, ComputeModel, ComputeScheduler, OperationClass};
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
use program::PreparedProgram;
use program::validate::RegionBindings;
use serde::{Deserialize, Serialize};
use task::{PreparedTask, ProgramTaskConfig, Task};

use crate::log_stats;
use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::MemoryMap;
use crate::processing_element::dispatch::Dispatch;
use crate::processing_element::flop_monitor::FlopMonitor;
use crate::processing_element::load_store_unit::LoadStoreUnit;

mod activity;
use activity::{ActivityLanes, ProcessingElementActivityLanes};

pub mod compute;
pub mod dispatch;
mod flop_monitor;
mod load_store_unit;
pub mod program;
mod runner;
pub use program::analysis::estimate_peak_tile_sram;
use runner::TaskRunner;

pub mod task;
pub mod tensors;

#[derive(EntityGet, EntityDisplay)]
pub struct ProcessingElement {
    entity: Rc<Entity>,
    lsu: Rc<LoadStoreUnit>,
    clock: Clock,
    spawner: Spawner,

    compute_model: Rc<ComputeModel>,
    compute_scheduler: ComputeScheduler,
    sram: CapacityAllocator,
    sram_bytes: usize,
    stats: Rc<RefCell<ProcessingElementStats>>,
    activity_lanes: Rc<ProcessingElementActivityLanes>,
    dispatcher: RefCell<Option<Dispatcher>>,
    flop_monitor: Option<Rc<FlopMonitor>>,
}

#[async_trait(?Send)]
impl Runnable for ProcessingElement {
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
                dispatcher.wait_for_change().await;
            } else {
                // Spawn all so they can run in parallel
                for task_idx in ready_node_indices.drain(..) {
                    let task = match dispatcher.task_by_id(task_idx)? {
                        Task::ProgramTask { config } => self.prepare_program_task(*config)?,
                        Task::PreparedProgram { task } => {
                            if !task.belongs_to(&self.entity) {
                                return sim_error!(
                                    "prepared task belongs to another processing element"
                                );
                            }
                            task
                        }
                        Task::SyncTask { .. } => {
                            return sim_error!("synchronization tasks are unsupported");
                        }
                    };
                    dispatcher.set_task_active(task_idx)?;

                    let runner = TaskRunner {
                        entity: self.entity.clone(),
                        clock: self.clock.clone(),
                        dispatcher: dispatcher.clone(),
                        lsu: self.lsu.clone(),
                        #[cfg(test)]
                        compute_model: self.compute_model.clone(),
                        compute_scheduler: self.compute_scheduler.clone(),
                        sram: self.sram.clone(),
                        sram_bytes: self.sram_bytes,
                        stats: self.stats.clone(),
                        activity_lanes: self.activity_lanes.clone(),
                        flop_monitor: self.flop_monitor.clone(),
                    };
                    self.spawner
                        .spawn(async move { runner.run(task_idx, task).await });
                }
            }

            (complete, ready_node_indices) = dispatcher.ready_task_indices(pe_name)?;
        }
        debug!(self.entity ; "PE {pe_name} DONE");
        Ok(())
    }
}

impl ProcessingElement {
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
        let rc_self = Rc::new(Self {
            entity: entity.clone(),
            lsu,
            clock: clock.clone(),
            spawner: engine.spawner(),

            compute_model,
            compute_scheduler,
            sram,
            sram_bytes: pe_config.sram_bytes,
            stats: Rc::new(RefCell::new(ProcessingElementStats::default())),
            activity_lanes,

            dispatcher: RefCell::new(None),
            flop_monitor,
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    /// Effective hardware settings retained by this PE after default
    /// resolution.
    #[must_use]
    pub fn configuration_description(&self) -> String {
        format!(
            "SRAM capacity: {} bytes; {}; execution units: {}",
            self.sram_bytes,
            self.lsu.configuration_description(),
            self.compute_model.execution_unit_names().join(", "),
        )
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

    /// Bind raw syntax and prepare it for this exact PE, without side effects.
    pub fn prepare_program_task(
        &self,
        config: ProgramTaskConfig,
    ) -> Result<Rc<PreparedTask>, SimError> {
        let program = Rc::new(PreparedProgram::new(
            &config.program,
            RegionBindings {
                inputs: config.inputs,
                outputs: config.outputs,
            },
            config.tiling.as_ref(),
        )?);
        self.prepare_program(config.id, program)
    }

    /// Immutable compute capabilities used for workload selection.
    pub fn compute_model(&self) -> &ComputeModel {
        &self.compute_model
    }

    /// Physical regions addressable by this processing element.
    pub fn memory_map(&self) -> &MemoryMap {
        self.lsu.memory_map()
    }

    /// Check the actual touched bytes of a view against this PE's map.
    pub fn validate_mapped_view(&self, view: &tensors::TensorView) -> SimResult {
        self.lsu.validate_mapped_view(view)
    }

    /// Prepare a shared bound program for this exact PE instance.
    ///
    /// Checks actual invocations, durations, touched memory mappings and SRAM
    /// capacity before activation, without issuing requests. Retain the
    /// returned handle for dispatch and accounting; it cannot execute on
    /// another PE.
    pub fn prepare_program(
        &self,
        id: String,
        program: Rc<PreparedProgram>,
    ) -> Result<Rc<PreparedTask>, SimError> {
        PreparedTask::prepare(
            id,
            program,
            self.entity.clone(),
            &self.lsu,
            &self.compute_model,
            self.sram_bytes,
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
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
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

pub struct ProcessingElementConfig {
    /// The number of outstanding requests can the LSU handle at once
    pub num_active_requests: usize,

    /// Bytes in each aligned memory access. Every memory-map region's start
    /// and exclusive end must be aligned to this size; PE construction checks
    /// this requirement.
    pub lsu_access_bytes: usize,

    /// The number of bytes of protocol overhead for each memory transaction
    pub overhead_size_bytes: usize,

    /// The total number of local SRAM storage bytes available to the PE
    pub sram_bytes: usize,

    /// Typed execution units.
    pub compute: ComputeConfig,
    pub operation_classes: BTreeMap<String, OperationClass>,
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
        ComputeModel::new(&self.compute, &self.operation_classes)?;
        Ok(())
    }
}

#[derive(Default)]
struct ProcessingElementStats {
    machine_ops: MachineOpCounts,
    read_bytes: usize,
    written_bytes: usize,
    peak_sram_bytes: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MachineOpCounts {
    pub by_operation: BTreeMap<String, usize>,
    pub flop_equivalents: usize,
}

impl MachineOpCounts {
    pub(crate) fn single(op: &str, count: usize, flops_per_op: usize) -> Result<Self, SimError> {
        Ok(Self {
            by_operation: BTreeMap::from([(op.to_owned(), count)]),
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

#[cfg(test)]
mod matrix_test_fixture;
