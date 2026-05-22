// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A Processing Element (PE) for a simulation.
//!
//! The PE can model data load/stores as well as performing computations
//! as defined by a timetable.
//!
//! The PE comprises:
//!  - Load/Store
//!  - Internal Buffers
//!  - Compute
//!
//! Identifies all operation nodes (load/store/compute) that can execute
//! because their dependencies are satisfied (or they have no dependencies).

//! # Ports
//!
//! Each PE has:
//!  - One [input port](gwr_engine::port::InPort): `rx`
//!  - One [output port](gwr_engine::port::OutPort): `tx`
//!
//! that are managed by the `LoadStoreUnit`

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::port::PortStateResult;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::Runnable;
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;
use gwr_track::{debug, info};

use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::{DeviceId, MemoryMap};
use crate::processing_element::dispatch::Dispatch;
use crate::processing_element::flop_monitor::FlopMonitor;
use crate::processing_element::load_store_unit::LoadStoreUnit;
use crate::processing_element::operators::{MachineOp, MachineOps, TensorView};
use crate::processing_element::task::{ComputeTaskConfig, MemoryOp, MemoryTaskConfig, Task};

pub mod dispatch;
mod flop_monitor;
mod load_store_unit;
pub mod operators;
pub mod task;

type Dispatcher = Rc<dyn Dispatch>;

pub struct ProcessingElementConfig {
    /// The number of outstanding requests can the LSU handle at once
    pub num_active_requests: usize,

    /// The maximum number of bytes in each memory access
    pub lsu_access_bytes: usize,

    /// The number of bytes of protocol overhead for each memory transaction
    pub overhead_size_bytes: usize,

    /// The total number of local SRAM storage bytes available to the PE
    pub sram_bytes: usize,

    /// Number of add operations per tick
    pub adds_per_tick: f64,

    /// Number of multiply operations per tick
    pub muls_per_tick: f64,

    /// Number of compare operations per tick
    pub compares_per_tick: f64,
}

pub struct ComputeCapabilities {
    adds_per_tick: f64,
    muls_per_tick: f64,
    compares_per_tick: f64,
    sram_bytes: usize,
}

impl ComputeCapabilities {
    #[must_use]
    pub fn ops_per_tick(&self, op: MachineOp) -> f64 {
        match op {
            MachineOp::Add => self.adds_per_tick,
            MachineOp::Compare => self.compares_per_tick,
            MachineOp::Mul => self.muls_per_tick,
        }
    }

    pub fn cycles_for_ops(&self, num_ops: usize, op: MachineOp) -> Result<usize, SimError> {
        let ops_per_tick = self.ops_per_tick(op);
        if !ops_per_tick.is_finite() || ops_per_tick <= 0.0 {
            return Err(SimError(format!(
                "invalid compute throughput {ops_per_tick} ops/tick"
            )));
        }

        Ok(((num_ops as f64) / ops_per_tick).ceil() as usize)
    }
}

#[derive(Default)]
struct ProcessingElementStats {
    total_flops: usize,
}

#[derive(EntityGet, EntityDisplay)]
pub struct ProcessingElement {
    entity: Rc<Entity>,
    lsu: Rc<LoadStoreUnit>,
    clock: Clock,
    spawner: Spawner,

    compute_capabilities: Rc<ComputeCapabilities>,
    stats: Rc<RefCell<ProcessingElementStats>>,
    dispatcher: RefCell<Option<Dispatcher>>,
    flop_monitor: Option<Rc<FlopMonitor>>,
}

impl ProcessingElement {
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
        let entity = Rc::new(Entity::new(parent, name));

        let lsu = LoadStoreUnit::new_and_register(
            engine, clock, &entity, aka, pe_config, memory_map, device_id,
        )?;
        let monitor_window_size = entity.tracker.monitoring_window_size_for(entity.id);
        let flop_monitor = monitor_window_size.map(|window_size_ticks| {
            FlopMonitor::new_and_register(engine, &entity, clock, window_size_ticks)
        });

        let rc_self = Rc::new(Self {
            entity,
            lsu,
            clock: clock.clone(),
            spawner: engine.spawner(),

            compute_capabilities: Rc::new(ComputeCapabilities {
                adds_per_tick: pe_config.adds_per_tick,
                muls_per_tick: pe_config.muls_per_tick,
                compares_per_tick: pe_config.compares_per_tick,
                sram_bytes: pe_config.sram_bytes,
            }),
            stats: Rc::new(RefCell::new(ProcessingElementStats::default())),

            dispatcher: RefCell::new(None),
            flop_monitor,
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
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
    pub fn compute_capabilities(&self) -> &ComputeCapabilities {
        self.compute_capabilities.as_ref()
    }

    #[must_use]
    pub fn lsu_access_bytes_per_tick(&self) -> usize {
        self.lsu.max_access_size_bytes()
    }

    #[must_use]
    pub fn can_access_addr(&self, addr: u64) -> bool {
        self.lsu.can_access_addr(addr)
    }

    pub fn compute_ticks_for_ops(&self, machine_ops: &MachineOps) -> Result<f64, SimError> {
        let mut total = 0.0;
        for machine_op in MachineOp::ALL {
            let Some(count) = machine_ops.get(&machine_op) else {
                continue;
            };
            let ops_per_tick = self.compute_capabilities.ops_per_tick(machine_op);
            if !ops_per_tick.is_finite() || ops_per_tick <= 0.0 {
                return Err(SimError(format!(
                    "{}: invalid compute throughput {ops_per_tick} ops/tick for {machine_op}",
                    self.entity.name
                )));
            }
            total += (*count as f64) / ops_per_tick;
        }
        Ok(total)
    }

    #[must_use]
    pub fn total_graph_nodes(&self) -> usize {
        match self.dispatcher.borrow().as_ref() {
            None => 0,
            Some(dispatcher) => dispatcher.total_tasks_for_pe(self.entity.name.as_str()),
        }
    }

    pub fn dump_stats(&self, time_now_ns: f64) {
        let stats = self.stats.borrow();
        let time_now_s = time_now_ns / 1e9;
        let total_gflops = stats.total_flops as f64 / 1e9;
        let average_gflops_per_second = if time_now_s == 0.0 {
            0.0
        } else {
            total_gflops / time_now_s
        };

        info!(self.entity ; "ProcessingElement {}:", self.entity.full_name());
        info!(self.entity ;
            "  FLOPs: {}, {total_gflops:.2} GFLOPs, {average_gflops_per_second:.2} GFLOP/s",
            stats.total_flops
        );
    }
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
                    dispatcher.set_task_active(task_idx)?;

                    let clock = self.clock.clone();
                    let dispatcher = dispatcher.clone();
                    let lsu = self.lsu.clone();
                    let compute_capabilities = self.compute_capabilities.clone();
                    let stats = self.stats.clone();
                    let entity = self.entity.clone();
                    let flop_monitor = self.flop_monitor.clone();
                    self.spawner.spawn(async move {
                        handle_task(
                            entity,
                            clock,
                            dispatcher,
                            lsu,
                            compute_capabilities,
                            stats,
                            flop_monitor,
                            task_idx,
                        )
                        .await
                    });
                }
            }

            (complete, ready_node_indices) = dispatcher.ready_task_indices(pe_name)?;
        }
        debug!(self.entity ; "PE {pe_name} DONE");
        Ok(())
    }
}

#[expect(clippy::too_many_arguments)]
async fn handle_task(
    entity: Rc<Entity>,
    clock: Clock,
    dispatcher: Dispatcher,
    lsu: Rc<LoadStoreUnit>,
    compute_capabilities: Rc<ComputeCapabilities>,
    stats: Rc<RefCell<ProcessingElementStats>>,
    flop_monitor: Option<Rc<FlopMonitor>>,
    task_idx: usize,
) -> SimResult {
    let task = dispatcher.task_by_id(task_idx)?;
    match task {
        Task::ComputeTask { config } => handle_compute_task(
            clock,
            dispatcher,
            lsu,
            task_idx,
            compute_capabilities,
            stats,
            flop_monitor,
            &config,
        )
        .await
        .map_err(|err| SimError(format!("{entity} had error on task {}:\n{err}", config.id))),
        Task::MemoryTask { config } => handle_memory_task(dispatcher, lsu, task_idx, &config)
            .await
            .map_err(|err| SimError(format!("{entity} had error on task {}:\n{err}", config.id))),
        Task::SyncTask { .. } => {
            todo!();
        }
    }
}

fn tensor_view_num_bytes(view: &TensorView) -> usize {
    view.num_bytes()
}

fn tensor_view_base_addr(view: &TensorView) -> Result<u64, SimError> {
    let base_addr = view.tensor().addr();
    let element_offset = view.element_offset()?;
    let dtype = view.tensor().dtype();
    let byte_offset = (dtype.num_bits() * element_offset).div_ceil(8) as u64;
    Ok(base_addr + byte_offset)
}

#[expect(clippy::too_many_arguments)]
async fn handle_compute_task(
    clock: Clock,
    dispatcher: Dispatcher,
    lsu: Rc<LoadStoreUnit>,
    task_idx: usize,
    compute_capabilities: Rc<ComputeCapabilities>,
    stats: Rc<RefCell<ProcessingElementStats>>,
    flop_monitor: Option<Rc<FlopMonitor>>,
    config: &ComputeTaskConfig,
) -> SimResult {
    let total_num_bytes: usize = config
        .inputs
        .iter()
        .chain(config.outputs.iter())
        .flatten()
        .map(tensor_view_num_bytes)
        .sum();

    let num_partitions = total_num_bytes
        .div_ceil(compute_capabilities.sram_bytes.max(1))
        .max(1);

    let partitions =
        config
            .op
            .create_partitions(&config.inputs, &config.outputs, num_partitions)?;

    for partition in partitions {
        for view in partition.inputs.iter().flatten() {
            lsu.do_access(
                AccessType::ReadRequest,
                tensor_view_num_bytes(view),
                tensor_view_base_addr(view)?,
            )
            .await?;
        }

        let compute_ticks = config.op.compute_delay_ticks(
            &compute_capabilities,
            &partition.inputs,
            &partition.outputs,
        )?;
        let compute_flops = config
            .op
            .compute_flops(&partition.inputs, &partition.outputs)?;
        if let Some(flop_monitor) = &flop_monitor {
            flop_monitor.record_interval(compute_ticks as u64, compute_flops.total_flops() as f64);
        }
        clock.wait_ticks(compute_ticks as u64).await;
        stats.borrow_mut().total_flops += compute_flops.total_flops();

        for view in partition.outputs.iter().flatten() {
            lsu.do_access(
                AccessType::WriteNonPostedRequest,
                tensor_view_num_bytes(view),
                tensor_view_base_addr(view)?,
            )
            .await?;
        }
    }

    dispatcher.set_task_completed(task_idx)
}

// Spawn the handling of memory nodes so that thye can run in parallel.
async fn handle_memory_task(
    dispatcher: Dispatcher,
    lsu: Rc<LoadStoreUnit>,
    task_idx: usize,
    config: &MemoryTaskConfig,
) -> SimResult {
    let dst_addr = config.addr;
    let access_type = match config.op {
        MemoryOp::Load => AccessType::ReadRequest,
        MemoryOp::Store => AccessType::WriteNonPostedRequest,
    };

    let access_size_bytes = config.num_bytes;
    lsu.do_access(access_type, access_size_bytes, dst_addr)
        .await?;
    dispatcher.set_task_completed(task_idx)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_for_ops_uses_ceil_for_fractional_throughput() {
        let compute_capabilities = ComputeCapabilities {
            adds_per_tick: 0.5,
            muls_per_tick: 2.5,
            compares_per_tick: 4.0,
            sram_bytes: 1024,
        };

        assert_eq!(
            compute_capabilities
                .cycles_for_ops(3, MachineOp::Add)
                .unwrap(),
            6
        );
        assert_eq!(
            compute_capabilities
                .cycles_for_ops(6, MachineOp::Mul)
                .unwrap(),
            3
        );
        assert_eq!(
            compute_capabilities
                .cycles_for_ops(0, MachineOp::Compare)
                .unwrap(),
            0
        );
    }

    #[test]
    fn cycles_for_ops_rejects_invalid_throughput() {
        let compute_capabilities = ComputeCapabilities {
            adds_per_tick: 0.0,
            muls_per_tick: -1.0,
            compares_per_tick: f64::INFINITY,
            sram_bytes: 1024,
        };

        assert!(
            compute_capabilities
                .cycles_for_ops(1, MachineOp::Add)
                .is_err()
        );
        assert!(
            compute_capabilities
                .cycles_for_ops(1, MachineOp::Mul)
                .is_err()
        );
        assert!(
            compute_capabilities
                .cycles_for_ops(1, MachineOp::Compare)
                .is_err()
        );

        let compute_capabilities = ComputeCapabilities {
            adds_per_tick: f64::NAN,
            muls_per_tick: 1.0,
            compares_per_tick: 1.0,
            sram_bytes: 1024,
        };

        assert!(
            compute_capabilities
                .cycles_for_ops(1, MachineOp::Add)
                .is_err()
        );
    }
}
