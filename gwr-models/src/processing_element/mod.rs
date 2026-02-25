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
use gwr_track::debug;
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;

use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::{DeviceId, MemoryMap};
use crate::processing_element::dispatch::Dispatch;
use crate::processing_element::load_store_unit::LoadStoreUnit;
use crate::processing_element::task::{
    ComputeOp, ComputeTaskConfig, MemoryOp, MemoryTaskConfig, Task,
};

pub mod dispatch;
mod load_store_unit;
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
    pub adds_per_tick: usize,

    /// Number of multiply operations per tick
    pub muls_per_tick: usize,
}

struct ComputeTimings {
    adds_per_tick: usize,
    muls_per_tick: usize,
}

#[derive(EntityGet, EntityDisplay)]
pub struct ProcessingElement {
    entity: Rc<Entity>,
    lsu: Rc<LoadStoreUnit>,
    clock: Clock,
    spawner: Spawner,

    compute_timings: Rc<ComputeTimings>,
    dispatcher: RefCell<Option<Dispatcher>>,
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

        let rc_self = Rc::new(Self {
            entity,
            lsu,
            clock: clock.clone(),
            spawner: engine.spawner(),

            compute_timings: Rc::new(ComputeTimings {
                adds_per_tick: pe_config.adds_per_tick,
                muls_per_tick: pe_config.muls_per_tick,
            }),

            dispatcher: RefCell::new(None),
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
    pub fn total_graph_nodes(&self) -> usize {
        match self.dispatcher.borrow().as_ref() {
            None => 0,
            Some(dispatcher) => dispatcher.total_tasks_for_pe(self.entity.name.as_str()),
        }
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
                    let compute_timings = self.compute_timings.clone();
                    self.spawner.spawn(async move {
                        handle_task(clock, dispatcher, lsu, compute_timings, task_idx).await
                    });
                }
            }

            (complete, ready_node_indices) = dispatcher.ready_task_indices(pe_name)?;
        }
        debug!(self.entity ; "PE {pe_name} DONE");
        Ok(())
    }
}

async fn handle_task(
    clock: Clock,
    dispatcher: Dispatcher,
    lsu: Rc<LoadStoreUnit>,
    compute_timings: Rc<ComputeTimings>,
    task_idx: usize,
) -> SimResult {
    let task = dispatcher.task_by_id(task_idx)?;
    match task {
        Task::ComputeTask { config } => {
            handle_compute_task(clock, dispatcher, task_idx, compute_timings, &config).await
        }
        Task::MemoryTask { config } => handle_memory_task(dispatcher, lsu, task_idx, &config).await,
        Task::SyncTask { region: _ } => {
            // TODO
            dispatcher.set_task_completed(task_idx)
        }
    }
}

async fn handle_compute_task(
    clock: Clock,
    dispatcher: Dispatcher,
    task_idx: usize,
    compute_timings: Rc<ComputeTimings>,
    config: &ComputeTaskConfig,
) -> SimResult {
    let ops_per_tick = match config.op {
        ComputeOp::Add => compute_timings.adds_per_tick,
        ComputeOp::Mul => compute_timings.muls_per_tick,
    };
    let compute_ticks = config.num_ops.div_ceil(ops_per_tick);
    clock.wait_ticks(compute_ticks as u64).await;
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
