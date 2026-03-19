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
use crate::processing_element::operators::TensorView;
use crate::processing_element::task::{ComputeTaskConfig, MemoryOp, MemoryTaskConfig, Task};

pub mod dispatch;
mod load_store_unit;
pub mod operators;
pub mod task;

#[derive(Eq, Hash, PartialEq)]
pub enum MachineOp {
    Add,
    Mul,
}

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

pub struct ComputeCapabilities {
    adds_per_tick: usize,
    muls_per_tick: usize,
    sram_bytes: usize,
}

#[derive(EntityGet, EntityDisplay)]
pub struct ProcessingElement {
    entity: Rc<Entity>,
    lsu: Rc<LoadStoreUnit>,
    clock: Clock,
    spawner: Spawner,

    compute_capabilities: Rc<ComputeCapabilities>,
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

            compute_capabilities: Rc::new(ComputeCapabilities {
                adds_per_tick: pe_config.adds_per_tick,
                muls_per_tick: pe_config.muls_per_tick,
                sram_bytes: pe_config.sram_bytes,
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
                    let compute_capabilities = self.compute_capabilities.clone();
                    let entity = self.entity.clone();
                    self.spawner.spawn(async move {
                        handle_task(
                            entity,
                            clock,
                            dispatcher,
                            lsu,
                            compute_capabilities,
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

async fn handle_task(
    entity: Rc<Entity>,
    clock: Clock,
    dispatcher: Dispatcher,
    lsu: Rc<LoadStoreUnit>,
    compute_capabilities: Rc<ComputeCapabilities>,
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
            &config,
        )
        .await
        .map_err(|err| SimError(format!("{entity} had error on task {}:\n{err}", config.id))),
        Task::MemoryTask { config } => handle_memory_task(dispatcher, lsu, task_idx, &config)
            .await
            .map_err(|err| SimError(format!("{entity} had error on task {}:\n{err}", config.id))),
        Task::SyncTask { region: _ } => {
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

async fn handle_compute_task(
    clock: Clock,
    dispatcher: Dispatcher,
    lsu: Rc<LoadStoreUnit>,
    task_idx: usize,
    compute_capabilities: Rc<ComputeCapabilities>,
    config: &ComputeTaskConfig,
) -> SimResult {
    let total_num_bytes: usize = config
        .inputs
        .iter()
        .chain(config.outputs.iter())
        .filter_map(|view| view.as_ref())
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
        clock.wait_ticks(compute_ticks as u64).await;

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
