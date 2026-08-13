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
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::port::PortStateResult;
use gwr_engine::sim_error;
use gwr_engine::time::clock::{Clock, phase};
use gwr_engine::traits::Runnable;
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::debug;
use gwr_track::entity::{Entity, EntityGroup, EntityLane};
use gwr_track::tracker::aka::Aka;
use serde::{Deserialize, Serialize};

use crate::log_stats;
use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::{DeviceId, MemoryMap};
use crate::processing_element::dispatch::Dispatch;
use crate::processing_element::flop_monitor::FlopMonitor;
use crate::processing_element::load_store_unit::LoadStoreUnit;
use crate::processing_element::operators::TensorView;
use crate::processing_element::task::{ComputeTaskConfig, Task};

pub mod dispatch;
mod flop_monitor;
mod load_store_unit;
pub mod operators;
pub mod task;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub enum MachineOp {
    Add,
    Compare,
    Mul,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MachineOpCounts {
    #[serde(default)]
    pub adds: usize,
    #[serde(default)]
    pub compares: usize,
    #[serde(default)]
    pub muls: usize,
}

impl MachineOpCounts {
    #[must_use]
    pub fn total(&self) -> usize {
        self.adds + self.compares + self.muls
    }

    pub fn add_assign(&mut self, other: Self) {
        self.adds += other.adds;
        self.compares += other.compares;
        self.muls += other.muls;
    }
}

pub struct ProcessingElementStatsDisplay {
    prefix: String,
    time_now_ns: f64,
    machine_ops: MachineOpCounts,
}

impl ProcessingElementStatsDisplay {
    #[must_use]
    pub fn new(prefix: impl Into<String>, time_now_ns: f64, machine_ops: MachineOpCounts) -> Self {
        Self {
            prefix: prefix.into(),
            time_now_ns,
            machine_ops,
        }
    }
}

impl Display for ProcessingElementStatsDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total_flops = self.machine_ops.total();
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
        write!(
            f,
            "  Machine ops: {} total, {} add, {} mul, {} compare",
            self.machine_ops.total(),
            self.machine_ops.adds,
            self.machine_ops.muls,
            self.machine_ops.compares
        )
    }
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
        if num_ops == 0 {
            return Ok(0);
        }

        let ops_per_tick = self.ops_per_tick(op);
        if !ops_per_tick.is_finite() || ops_per_tick <= 0.0 {
            return sim_error!("invalid compute throughput {ops_per_tick} ops/tick");
        }

        Ok(((num_ops as f64) / ops_per_tick).ceil() as usize)
    }
}

#[derive(Default)]
struct ProcessingElementStats {
    machine_ops: MachineOpCounts,
}

struct Lane {
    lane: EntityLane,
    active: bool,
}

pub(crate) struct ActivityLanes {
    entity: Rc<Entity>,
    track_name: String,
    lanes: Vec<Lane>,
}

impl ActivityLanes {
    fn new(entity: Rc<Entity>, track_name: &str) -> Self {
        Self {
            entity,
            track_name: track_name.to_string(),
            lanes: Vec::new(),
        }
    }

    fn begin_in_group(
        lanes: &Rc<RefCell<Self>>,
        name: &str,
        group: &Rc<EntityGroup>,
    ) -> ActivityLaneGuard {
        let mut lanes_ref = lanes.borrow_mut();
        let lane_idx = match lanes_ref.lanes.iter().position(|lane| !lane.active) {
            Some(lane_idx) => lane_idx,
            None => lanes_ref.add_new_lane(),
        };

        let lane = &mut lanes_ref.lanes[lane_idx];
        lane.lane.begin_in_group(name, group);
        lane.active = true;

        ActivityLaneGuard {
            lanes: lanes.clone(),
            lane_idx,
            active: true,
        }
    }

    fn add_new_lane(&mut self) -> usize {
        let lane_idx = self.lanes.len();
        let lane = EntityLane::new(&self.entity, &format!("{}::{lane_idx}", self.track_name));
        self.lanes.push(Lane {
            lane,
            active: false,
        });
        lane_idx
    }

    fn end(&mut self, lane_idx: usize) {
        let lane = &mut self.lanes[lane_idx];
        lane.lane.end();
        lane.active = false;
    }
}

struct ActivityLaneGuard {
    lanes: Rc<RefCell<ActivityLanes>>,
    lane_idx: usize,
    active: bool,
}

impl Drop for ActivityLaneGuard {
    fn drop(&mut self) {
        if self.active {
            self.lanes.borrow_mut().end(self.lane_idx);
            self.active = false;
        }
    }
}

struct ProcessingElementActivityLanes {
    entity: Rc<Entity>,
    compute: Rc<RefCell<ActivityLanes>>,
    lsu_read: Rc<RefCell<ActivityLanes>>,
    lsu_write: Rc<RefCell<ActivityLanes>>,
}

impl ProcessingElementActivityLanes {
    fn new(entity: Rc<Entity>) -> Self {
        Self {
            entity: entity.clone(),
            compute: Rc::new(RefCell::new(ActivityLanes::new(
                entity.clone(),
                "lane::compute",
            ))),
            lsu_read: Rc::new(RefCell::new(ActivityLanes::new(
                entity.clone(),
                "lane::lsu_read",
            ))),
            lsu_write: Rc::new(RefCell::new(ActivityLanes::new(entity, "lane::lsu_write"))),
        }
    }

    fn create_group(&self, name: &str) -> Rc<EntityGroup> {
        Rc::new(EntityGroup::new(&self.entity, name))
    }
}

#[derive(EntityGet, EntityDisplay)]
pub struct ProcessingElement {
    entity: Rc<Entity>,
    lsu: Rc<LoadStoreUnit>,
    clock: Clock,
    spawner: Spawner,

    compute_capabilities: Rc<ComputeCapabilities>,
    stats: Rc<RefCell<ProcessingElementStats>>,
    activity_lanes: Rc<ProcessingElementActivityLanes>,
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
            entity: entity.clone(),
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
            activity_lanes: Rc::new(ProcessingElementActivityLanes::new(entity.clone())),

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
    pub fn total_graph_nodes(&self) -> usize {
        match self.dispatcher.borrow().as_ref() {
            None => 0,
            Some(dispatcher) => dispatcher.total_tasks_for_pe(self.entity.name.as_str()),
        }
    }

    #[must_use]
    pub fn total_flops(&self) -> usize {
        self.stats.borrow().machine_ops.total()
    }

    #[must_use]
    pub fn machine_ops(&self) -> MachineOpCounts {
        self.stats.borrow().machine_ops
    }

    pub fn dump_stats(&self, time_now_ns: f64) {
        let stats = self.stats.borrow();
        log_stats(
            &self.entity,
            ProcessingElementStatsDisplay::new(
                format!("ProcessingElement {}", self.entity.full_name()),
                time_now_ns,
                stats.machine_ops,
            ),
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
                    let activity_lanes = self.activity_lanes.clone();
                    let flop_monitor = self.flop_monitor.clone();
                    self.spawner.spawn(async move {
                        handle_task(
                            entity,
                            clock,
                            dispatcher,
                            lsu,
                            compute_capabilities,
                            stats,
                            activity_lanes,
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
    activity_lanes: Rc<ProcessingElementActivityLanes>,
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
            activity_lanes,
            flop_monitor,
            &config,
        )
        .await
        .map_err(|err| SimError(format!("{entity} had error on task {}:\n{err}", config.id))),
        Task::SyncTask { .. } => {
            todo!();
        }
    }
}

fn tensor_view_access(view: &TensorView) -> Result<(u64, usize), SimError> {
    let byte_range = view.byte_range()?;
    let byte_offset = u64::try_from(byte_range.start)
        .map_err(|error| SimError(format!("Tensor view byte offset: {error}")))?;
    let address = view
        .tensor()
        .addr()
        .checked_add(byte_offset)
        .ok_or_else(|| SimError("Tensor view address overflows".to_string()))?;
    Ok((address, byte_range.end - byte_range.start))
}

#[expect(clippy::too_many_arguments)]
async fn handle_compute_task(
    clock: Clock,
    dispatcher: Dispatcher,
    lsu: Rc<LoadStoreUnit>,
    task_idx: usize,
    compute_capabilities: Rc<ComputeCapabilities>,
    stats: Rc<RefCell<ProcessingElementStats>>,
    activity_lanes: Rc<ProcessingElementActivityLanes>,
    flop_monitor: Option<Rc<FlopMonitor>>,
    config: &ComputeTaskConfig,
) -> SimResult {
    let total_num_bytes: usize = config
        .inputs
        .iter()
        .chain(config.outputs.iter())
        .filter_map(|view| view.as_ref())
        .map(TensorView::num_bytes)
        .sum();

    let num_partitions = total_num_bytes
        .div_ceil(compute_capabilities.sram_bytes.max(1))
        .max(1);

    let partitions =
        config
            .op
            .create_partitions(&config.inputs, &config.outputs, num_partitions)?;
    let activity_name = config.activity_name();
    let group = activity_lanes.create_group(&format!("{activity_name} operation"));

    for partition in partitions {
        for (idx, view) in partition.inputs.iter().enumerate() {
            let Some(view) = view else {
                continue;
            };
            let (address, num_bytes) = tensor_view_access(view)?;
            lsu.do_access(
                AccessType::ReadRequest,
                num_bytes,
                address,
                &activity_lanes.lsu_read,
                &format!("{activity_name} tensor {idx} read"),
                &group,
            )
            .await?;
        }

        let compute_ticks = config.op.compute_delay_ticks(
            &compute_capabilities,
            &partition.inputs,
            &partition.outputs,
        )?;
        let machine_ops = config
            .op
            .compute_machine_ops(&partition.inputs, &partition.outputs)?;
        let compute_flops = machine_ops.total();
        if let Some(flop_monitor) = &flop_monitor {
            flop_monitor.record_interval(compute_ticks as u64, compute_flops as f64);
        }
        {
            // Lanes cannot support overlapping activity. If a lane will be released
            // in the current clock cycle then we want to re-use it rather than allocate
            // a new lane. Hence we wait here for the end of the current clock cycle
            // to ensure all lanes that will be released in this cycle have been.
            clock.wait_phase(phase::END).await;

            let _activity = ActivityLanes::begin_in_group(
                &activity_lanes.compute,
                &format!("{activity_name} compute"),
                &group,
            );
            clock.wait_ticks(compute_ticks as u64).await;
        }
        stats.borrow_mut().machine_ops.add_assign(machine_ops);

        for (idx, view) in partition.outputs.iter().enumerate() {
            let Some(view) = view else {
                continue;
            };
            let (address, num_bytes) = tensor_view_access(view)?;
            lsu.do_access(
                AccessType::WriteNonPostedRequest,
                num_bytes,
                address,
                &activity_lanes.lsu_write,
                &format!("{activity_name} tensor {idx} write"),
                &group,
            )
            .await?;
        }
    }

    dispatcher.set_task_completed(task_idx)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing_element::operators::Tensor;
    use crate::processing_element::operators::dtype::DataType;

    #[test]
    fn tensor_view_access_includes_each_partially_touched_byte() {
        let tensor = Tensor::new(&[4], &DataType::Int4, 0x1000);
        let view = TensorView::new(tensor, &[2], &[1]);

        assert_eq!(tensor_view_access(&view).unwrap(), (0x1000, 2));
    }

    #[test]
    fn tensor_view_access_is_empty_for_zero_element_view() {
        let tensor = Tensor::new(&[4], &DataType::Int4, 0x1000);
        let view = TensorView::new(tensor, &[0], &[1]);

        assert_eq!(view.byte_range().unwrap(), 0..0);
        assert_eq!(tensor_view_access(&view).unwrap(), (0x1000, 0));
    }

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
