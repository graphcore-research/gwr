// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Generic static programs share the ordinary assignment source. Bounded
//! workers own input generations, a compute arena and acknowledged outputs.
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use futures::channel::{mpsc, oneshot};
use futures::{SinkExt, StreamExt};
use gwr_components::capacity_allocator::CapacityAllocator;
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_track::entity::EntityGroup;

use super::block_pipeline::{Slot, Slots, Window};
use super::{ActivityLanes, TaskRunner};
use crate::processing_element::block_execution::BlockExecutionConfig;
use crate::processing_element::block_runtime::Stage;
use crate::processing_element::compute::{AccumulatorLease, ComputeModel, ExecutionPlan};
use crate::processing_element::program::pipeline::PipelineMode;
use crate::processing_element::program::syntax::ExprSpec;
use crate::processing_element::program::tile::{RegionTile, TileSchedule, plan_tiles};
use crate::processing_element::program::validate::{
    RegionBindings, ValidatedRegion, validate_program,
};
use crate::processing_element::task::ProgramTaskConfig;
use crate::processing_element::tensors::TensorView;

fn closed() -> SimError {
    SimError("program pipeline stage closed".into())
}
fn running(failed: &Cell<bool>) -> SimResult {
    if failed.get() { Err(closed()) } else { Ok(()) }
}
async fn observe_failure(
    future: impl std::future::Future<Output = SimResult>,
    failed: &Cell<bool>,
) -> SimResult {
    let result = future.await;
    if result.is_err() {
        failed.set(true);
    }
    result
}
fn add(a: usize, b: usize) -> Result<usize, SimError> {
    a.checked_add(b)
        .ok_or_else(|| SimError("pipeline storage overflows".into()))
}
fn mul(a: usize, b: usize) -> Result<usize, SimError> {
    a.checked_mul(b)
        .ok_or_else(|| SimError("pipeline storage overflows".into()))
}

pub(crate) struct ProgramPipelinePlan {
    region: ValidatedRegion,
    schedule: TileSchedule,
    pub input_slot_bytes: usize,
    pub output_slot_bytes: usize,
    pub working_bytes: usize,
    pub bytes: usize,
}
impl ProgramPipelinePlan {
    pub fn validate_capacity(&self, config: &ProgramTaskConfig, capacity: usize) -> SimResult {
        if self.bytes > capacity {
            let p = config.pipeline.as_ref().unwrap();
            return Err(SimError(format!(
                "{} pipeline requires {} SRAM bytes, available {capacity} ({} input slots x {}, {} output slots x {}, compute working {})",
                config.id,
                self.bytes,
                p.input_buffers,
                self.input_slot_bytes,
                p.output_buffers,
                self.output_slot_bytes,
                self.working_bytes
            )));
        }
        Ok(())
    }
}
/// Immutable admission shared by runtime, preflight and generator fitting.
/// The compute arena deliberately retains boundary operand copies and result
/// scratch; slot-to-arena and arena-to-output copies are modelled explicitly.
pub(crate) fn plan(
    config: &ProgramTaskConfig,
    model: &ComputeModel,
    limits: &BlockExecutionConfig,
) -> Result<ProgramPipelinePlan, SimError> {
    limits.validate()?;
    let p = config.pipeline.as_ref().ok_or_else(closed)?;
    p.validate()?;
    if config.program.local_round_policy.is_some()
        || !config.program.indexed_metadata_inputs.is_empty()
        || config.program.assignments.iter().any(|a| scaled(&a.expr))
    {
        return Err(SimError("generic pipeline does not support local_round_policy, indexed metadata or scaled matrix payloads".into()));
    }
    let region = validate_program(
        &config.program,
        &RegionBindings {
            inputs: config.inputs.clone(),
            outputs: config.outputs.clone(),
        },
    )?;
    let schedule = plan_tiles(&region, config.tiling.as_ref())?;
    super::preflight_program(model, &region, &schedule)?;
    let mut input_slot_bytes = 0;
    let mut output_slot_bytes = 0;
    let mut working_bytes = 0;
    let packed = config.inputs.iter().any(|v| v.dtype().num_bits() < 8);
    let inspect_all = config.program.assignments.iter().any(|a| {
        super::uses_leading_padding(&a.expr) || (packed && super::uses_reduction_window(&a.expr))
    });
    for tile in schedule.tiles() {
        let s = super::tile::tile_storage(config, &region, &tile, usize::MAX)?;
        // A packed view can start in the last nibble of a physical byte. Keep
        // an aligned guard region per operand, including its working copy.
        let packed_guard = if packed { s.input_bytes } else { 0 };
        for index in &region.assignment_order {
            assignment_handoffs(config, &region, &tile, *index, &s.inputs)?;
        }
        input_slot_bytes = input_slot_bytes.max(add(s.input_bytes, packed_guard)?);
        output_slot_bytes = output_slot_bytes.max(s.outputs.iter().try_fold(0, |n, v| {
            add(
                n,
                super::align8(if v.dtype().num_bits() < 8 {
                    mul(v.num_packed_bytes(), 2)?
                } else {
                    v.layout().num_access_bytes()
                })?,
            )
        })?);
        working_bytes = working_bytes.max(add(
            add(s.arena, packed_guard)?,
            if config.outputs.iter().any(|v| v.dtype().num_bits() < 8) {
                s.output_bytes
            } else {
                0
            },
        )?);
        if !inspect_all {
            break;
        }
    }
    let bytes = add(
        add(
            mul(p.input_buffers, input_slot_bytes)?,
            mul(p.output_buffers, output_slot_bytes)?,
        )?,
        working_bytes,
    )?;
    Ok(ProgramPipelinePlan {
        region,
        schedule,
        input_slot_bytes,
        output_slot_bytes,
        working_bytes,
        bytes,
    })
}
fn scaled(expr: &ExprSpec) -> bool {
    match expr {
        ExprSpec::MatrixMac {
            quantization, args, ..
        } => quantization.is_some() || args.iter().any(scaled),
        ExprSpec::Call { args, .. } => args.iter().any(scaled),
        ExprSpec::Reduce { value, .. } => scaled(value),
        ExprSpec::Read { .. } => false,
    }
}

// Local service convention: one read of each resident operand per assignment,
// and one commit of each completed result. Reduction updates do not invent
// per-MAC SRAM accesses. Boundary chunk consumption is charged by each step.
fn assignment_handoffs(
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    tile: &RegionTile,
    index: usize,
    inputs: &[TensorView],
) -> Result<(usize, usize), SimError> {
    let assignment = &config.program.assignments[index];
    let value_bytes = |name: &str| -> Result<usize, SimError> {
        let producer = config
            .program
            .assignments
            .iter()
            .find(|a| {
                a.output.as_deref() == Some(name) || a.indices_output.as_deref() == Some(name)
            })
            .ok_or_else(|| SimError(format!("missing local producer {name}")))?;
        let elements = producer.axes.iter().try_fold(1, |n, axis| {
            mul(
                n,
                tile.output_intervals
                    .get(axis)
                    .map_or(region.extent(axis).unwrap(), Range::len),
            )
        })?;
        Ok(mul(elements, region.value(name).unwrap().dtype.num_bits())?.div_ceil(8))
    };
    let mut names = std::collections::BTreeSet::new();
    super::collect_read_names(&assignment.expr, &mut names);
    let reads = names.iter().try_fold(0, |n, name| {
        let bytes = if let Some(i) = config.program.inputs.iter().position(|input| input == name) {
            if config.program.metadata_inputs.contains(name)
                || !super::tile::is_reduction_input(config, name)
            {
                inputs[i].layout().num_access_bytes()
            } else {
                0
            }
        } else {
            value_bytes(name)?
        };
        add(n, bytes)
    })?;
    let writes = assignment
        .output
        .iter()
        .chain(assignment.indices_output.iter())
        .try_fold(0, |n, name| add(n, value_bytes(name)?))?;
    Ok((reads, writes))
}

pub(super) struct Read {
    ranges: Vec<Range<u128>>,
    bytes: usize,
    name: String,
}
pub(super) enum Event {
    Begin(RegionTile),
    Read(Read),
    Compute(String, ExecutionPlan, bool),
    LocalRead(usize),
    AssignmentEnd(oneshot::Sender<()>, usize),
    End(Vec<TensorView>, oneshot::Sender<()>),
}
/// A semantic assignment source either executes immediately or emits bounded
/// events. It never duplicates expression, reduction or affine-work analysis.
pub(super) enum AssignmentIo<'a> {
    Live(&'a TaskRunner),
    Planned(mpsc::Sender<Event>),
}
impl AssignmentIo<'_> {
    async fn event(&mut self, event: Event) -> SimResult {
        match self {
            Self::Planned(tx) => tx.send(event).await.map_err(|_| closed()),
            Self::Live(_) => Err(SimError(
                "ordinary assignment cannot emit a pipeline event".into(),
            )),
        }
    }
    pub async fn staged_access_ranges(
        &mut self,
        kind: AccessType,
        ranges: impl IntoIterator<Item = Range<u128>>,
        bytes: usize,
        lanes: &Rc<RefCell<ActivityLanes>>,
        name: &str,
        group: &Rc<EntityGroup>,
    ) -> SimResult {
        match self {
            Self::Live(r) => {
                r.staged_access_ranges(kind, ranges, bytes, lanes, name, group)
                    .await
            }
            Self::Planned(tx) => tx
                .send(Event::Read(Read {
                    ranges: ranges.into_iter().collect(),
                    bytes,
                    name: name.into(),
                }))
                .await
                .map_err(|_| closed()),
        }
    }
    pub async fn run_program_plan(
        &mut self,
        config: &ProgramTaskConfig,
        tile: &RegionTile,
        name: &str,
        group: &Rc<EntityGroup>,
        plan: ExecutionPlan,
    ) -> SimResult {
        match self {
            Self::Live(r) => r.run_program_plan(config, tile, name, group, plan).await,
            Self::Planned(tx) => tx
                .send(Event::Compute(name.into(), plan, true))
                .await
                .map_err(|_| closed()),
        }
    }
}
struct InputStep<'a> {
    failed: &'a Cell<bool>,
    reads: Vec<Read>,
    name: String,
    plan: ExecutionPlan,
    consume_inputs: bool,
}
enum Ready {
    Begin(RegionTile),
    LocalRead(usize),
    Step {
        slot: Rc<Slot>,
        credit: gwr_components::capacity_allocator::CapacityReservation,
        bytes: usize,
        name: String,
        plan: ExecutionPlan,
        consume_inputs: bool,
        ack: oneshot::Sender<()>,
    },
    AssignmentEnd(oneshot::Sender<()>, usize),
    End(Vec<TensorView>, oneshot::Sender<()>),
}
struct Output {
    views: Vec<TensorView>,
    slot: Rc<Slot>,
    ack: oneshot::Sender<()>,
}

impl TaskRunner {
    #[expect(
        clippy::too_many_lines,
        reason = "keep the four joined workers and their channel ownership visible together"
    )]
    pub(super) async fn run_program_pipeline(
        &self,
        config: &ProgramTaskConfig,
        plan: ProgramPipelinePlan,
    ) -> SimResult {
        plan.validate_capacity(config, self.sram_bytes)?;
        let p = config.pipeline.as_ref().unwrap();
        let r = self.block_execution.as_ref().unwrap();
        let before = self.clock.tick_now().tick();
        let reservation = self.sram.reserve(plan.bytes).await?;
        {
            let mut s = r.snapshot.borrow_mut();
            s.sram_wait_ticks += self.clock.tick_now().tick() - before;
            s.input_buffers = p.input_buffers;
            s.output_buffers = p.output_buffers;
            s.input_slot_bytes = plan.input_slot_bytes;
            s.output_slot_bytes = plan.output_slot_bytes;
            s.compute_working_bytes = plan.working_bytes;
            s.accumulator_bytes = 0;
            s.reserved_window_bytes = add(s.reserved_window_bytes, plan.bytes)?;
            s.peak_reserved_window_bytes =
                s.peak_reserved_window_bytes.max(s.reserved_window_bytes);
        }
        {
            let mut stats = self.stats.borrow_mut();
            stats.peak_sram_bytes = stats.peak_sram_bytes.max(self.sram.used());
        }
        let window = Rc::new(Window {
            _reservation: reservation,
            resources: r.clone(),
            bytes: plan.bytes,
        });
        let mut inputs = Slots::new(p.input_buffers, window.clone(), false);
        let mut outputs = Slots::new(p.output_buffers, window.clone(), true);
        let ahead = CapacityAllocator::new(
            &self.entity,
            "program_prefetch",
            p.prefetch_distance + 1,
            "chunks",
        )?;
        let (source_tx, mut source_rx) = mpsc::channel(0);
        // Each outstanding step owns a prefetch credit. Allow its begin,
        // operand-read, assignment-end and tile-end markers alongside it,
        // plus one fence crossing, so metadata cannot block future loads.
        // Actual operand queues remain bounded by the real input credits.
        let ready_capacity = add(mul(add(p.prefetch_distance, 1)?, 5)?, 4)?;
        let (mut ready_tx, mut ready_rx) = mpsc::channel(ready_capacity);
        let (mut output_tx, mut output_rx) = mpsc::channel(p.output_buffers);
        let serial = p.mode == PipelineMode::Serial;
        let failed_flag = Cell::new(false);
        let failed = &failed_flag;
        let source = async move {
            let mut io = AssignmentIo::Planned(source_tx);
            for tile in plan.schedule.tiles() {
                running(failed)?;
                let storage = super::tile::tile_storage(config, &plan.region, &tile, usize::MAX)?;
                io.event(Event::Begin(tile.clone()))
                    .await
                    .map_err(|_| closed())?;
                let group = self
                    .activity_lanes
                    .create_group(&format!("{} tile {:?}", config.id, tile.output_intervals));
                let mut initial_reads = false;
                for (name, view) in config.program.inputs.iter().zip(&storage.inputs) {
                    if !config.program.metadata_inputs.contains(name)
                        && (super::tile::is_reduction_input(config, name)
                            || !config
                                .program
                                .assignments
                                .iter()
                                .any(|a| super::find_read(&a.expr, name).is_some()))
                    {
                        continue;
                    }
                    io.staged_access_ranges(
                        AccessType::ReadRequest,
                        view.address_ranges(),
                        view.layout().num_access_bytes(),
                        &self.activity_lanes.lsu_read,
                        &format!("{} {name} initial read", config.id),
                        &group,
                    )
                    .await?;
                    initial_reads = true;
                }
                if initial_reads {
                    io.event(Event::Compute(
                        "initial boundary copies".into(),
                        ExecutionPlan { stages: vec![] },
                        false,
                    ))
                    .await?;
                }
                for (position, index) in plan.region.assignment_order.iter().enumerate() {
                    let (read_bytes, write_bytes) =
                        assignment_handoffs(config, &plan.region, &tile, *index, &storage.inputs)?;
                    io.event(Event::LocalRead(read_bytes))
                        .await
                        .map_err(|_| closed())?;
                    self.emit_program_assignment(
                        config,
                        &plan.region,
                        &tile,
                        &group,
                        *index,
                        &mut io,
                    )
                    .await?;
                    let (done, wait) = oneshot::channel();
                    io.event(Event::AssignmentEnd(done, write_bytes))
                        .await
                        .map_err(|_| closed())?;
                    if position + 1 < plan.region.assignment_order.len() {
                        wait.await.map_err(|_| closed())?;
                    }
                }
                let (done, wait) = oneshot::channel();
                io.event(Event::End(storage.outputs, done))
                    .await
                    .map_err(|_| closed())?;
                if serial {
                    wait.await.map_err(|_| closed())?;
                }
            }
            Ok::<_, SimError>(())
        };
        let load = async move {
            let mut reads = Vec::new();
            let mut tile_id = 0usize;
            let mut step = 0usize;
            while let Some(event) = source_rx.next().await {
                running(failed)?;
                match event {
                    Event::Read(read) => reads.push(read),
                    Event::Compute(name, work, consume_inputs) => {
                        let name = format!("tile {tile_id} step {step} {name}");
                        step = add(step, 1)?;
                        self.load_program_step(
                            config,
                            &mut inputs,
                            &ahead,
                            &mut ready_tx,
                            InputStep {
                                failed,
                                reads: std::mem::take(&mut reads),
                                name,
                                plan: work,
                                consume_inputs,
                            },
                            plan.input_slot_bytes,
                        )
                        .await?;
                    }
                    Event::LocalRead(bytes) => ready_tx
                        .send(Ready::LocalRead(bytes))
                        .await
                        .map_err(|_| closed())?,
                    Event::AssignmentEnd(ack, write_bytes) => {
                        if !reads.is_empty() {
                            self.load_program_step(
                                config,
                                &mut inputs,
                                &ahead,
                                &mut ready_tx,
                                InputStep {
                                    failed,
                                    reads: std::mem::take(&mut reads),
                                    name: format!("tile {tile_id} step {step} copy"),
                                    plan: ExecutionPlan { stages: vec![] },
                                    consume_inputs: true,
                                },
                                plan.input_slot_bytes,
                            )
                            .await?;
                            step = add(step, 1)?;
                        }
                        ready_tx
                            .send(Ready::AssignmentEnd(ack, write_bytes))
                            .await
                            .map_err(|_| closed())?;
                    }
                    Event::Begin(tile) => {
                        step = 0;
                        ready_tx
                            .send(Ready::Begin(tile))
                            .await
                            .map_err(|_| closed())?;
                    }
                    Event::End(views, ack) => {
                        ready_tx
                            .send(Ready::End(views, ack))
                            .await
                            .map_err(|_| closed())?;
                        tile_id = add(tile_id, 1)?;
                    }
                }
            }
            Ok::<_, SimError>(())
        };
        let compute = async move {
            let mut tile = None;
            let mut context = None;
            let mut accumulator: Option<AccumulatorLease> = None;
            while let Some(ready) = ready_rx.next().await {
                running(failed)?;
                match ready {
                    Ready::Begin(next) => {
                        context = Some(r.compute.reserve(1).await?);
                        tile = Some(next);
                    }
                    Ready::Step {
                        slot,
                        credit,
                        bytes,
                        name,
                        plan,
                        consume_inputs,
                        ack,
                    } => {
                        let group = self.activity_lanes.create_group(&format!(
                            "{} tile {:?} {name} input slot {} generation {}",
                            config.id,
                            tile.as_ref().unwrap().output_intervals,
                            slot.id,
                            slot.generation
                        ));
                        // The working arena owns the operands after this copy.
                        r.stage_local(false, bytes).await?;
                        r.stage_local(true, bytes).await?;
                        if consume_inputs {
                            r.stage_local(false, bytes).await?;
                        }
                        self.run_pipeline_work(plan, &group, &mut accumulator, failed)
                            .await?;
                        drop(slot);
                        drop(credit);
                        let _ = ack.send(());
                    }
                    Ready::LocalRead(bytes) => r.stage_local(false, bytes).await?,
                    Ready::AssignmentEnd(ack, write_bytes) => {
                        r.stage_local(true, write_bytes).await?;
                        accumulator = None;
                        let _ = ack.send(());
                    }
                    Ready::End(views, tile_done) => {
                        let before = self.clock.tick_now().tick();
                        let slot = outputs.acquire().await?;
                        r.snapshot.borrow_mut().output_wait_ticks +=
                            self.clock.tick_now().tick() - before;
                        let bytes = views
                            .iter()
                            .try_fold(0, |n, v| add(n, v.layout().num_access_bytes()))?;
                        // Distinct result scratch remains in the working arena
                        // until the completed payload has moved to its slot.
                        let handoff = r.stage(Stage::Finalise);
                        r.stage_local(false, bytes).await?;
                        r.stage_local(true, bytes).await?;
                        handoff.complete();
                        let (ack, wait) = oneshot::channel();
                        output_tx
                            .send(Output { views, slot, ack })
                            .await
                            .map_err(|_| closed())?;
                        drop(context.take());
                        tile = None;
                        if serial {
                            wait.await.map_err(|_| closed())?;
                        }
                        let _ = tile_done.send(());
                    }
                }
            }
            Ok::<_, SimError>(())
        };
        let store = async move {
            while let Some(Output { views, slot, ack }) = output_rx.next().await {
                running(failed)?;
                let permit = r.store.reserve(1).await?;
                running(failed)?;
                let stage = Rc::new(r.stage(Stage::Store));
                let group = self.activity_lanes.create_group(&format!(
                    "{} output slot {} generation {}",
                    config.id, slot.id, slot.generation
                ));
                let custody: Rc<dyn std::any::Any> = Rc::new((slot, permit, stage.clone()));
                for view in views {
                    running(failed)?;
                    r.stage_local(false, view.layout().num_access_bytes())
                        .await?;
                    running(failed)?;
                    self.lsu
                        .access_ranges_retaining(
                            AccessType::WriteNonPostedRequest,
                            view.address_ranges(),
                            &self.activity_lanes.lsu_write,
                            &format!("program store {}", group.name),
                            &group,
                            custody.clone(),
                        )
                        .await?;
                    self.record_write(&view)?;
                }
                stage.complete();
                drop(custody);
                let _ = ack.send(());
            }
            Ok::<_, SimError>(())
        };
        // Each worker owns its channel end. Failure closes downstream/upstream
        // queues, while already-issued LSU workers retain the real custody.
        let results = futures::join!(
            observe_failure(source, failed),
            observe_failure(load, failed),
            observe_failure(compute, failed),
            observe_failure(store, failed)
        );
        let errors = [results.0, results.1, results.2, results.3]
            .into_iter()
            .filter_map(Result::err)
            .collect::<Vec<_>>();
        if let Some(error) = errors
            .iter()
            .find(|e| e.0 != closed().0)
            .or_else(|| errors.first())
        {
            return Err(SimError(error.0.clone()));
        }
        Ok(())
    }
    async fn load_program_step(
        &self,
        config: &ProgramTaskConfig,
        inputs: &mut Slots,
        ahead: &CapacityAllocator,
        tx: &mut mpsc::Sender<Ready>,
        step: InputStep<'_>,
        capacity: usize,
    ) -> SimResult {
        let InputStep {
            failed,
            reads,
            name,
            plan,
            consume_inputs,
        } = step;
        let serial = config.pipeline.as_ref().unwrap().mode == PipelineMode::Serial;
        let r = self.block_execution.as_ref().unwrap();
        let before = self.clock.tick_now().tick();
        let credit = ahead.reserve(1).await?;
        let slot = inputs.acquire().await?;
        r.snapshot.borrow_mut().input_wait_ticks += self.clock.tick_now().tick() - before;
        let bytes = reads.iter().try_fold(0, |n, v| add(n, v.bytes))?;
        if bytes > capacity {
            return Err(SimError(format!(
                "{} input chunk {bytes} exceeds planned slot {capacity}",
                config.id
            )));
        }
        // Resident-only work needs no external load service. It still obeys
        // the bounded step/slot ordering without waiting behind another load.
        if !reads.is_empty() {
            let permit = r.load.reserve(1).await?;
            running(failed)?;
            let stage = Rc::new(r.stage(Stage::Load));
            let group = self.activity_lanes.create_group(&format!(
                "{} {name} input slot {} generation {}",
                config.id, slot.id, slot.generation
            ));
            let custody: Rc<dyn std::any::Any> = Rc::new((slot.clone(), permit, stage.clone()));
            for read in reads {
                running(failed)?;
                self.lsu
                    .access_ranges_retaining(
                        AccessType::ReadRequest,
                        read.ranges,
                        &self.activity_lanes.lsu_read,
                        &format!("program load {} {}", group.name, read.name),
                        &group,
                        custody.clone(),
                    )
                    .await?;
                self.record_read_bytes(read.bytes)?;
                running(failed)?;
                r.stage_local(true, read.bytes).await?;
            }
            stage.complete();
            drop(custody);
            drop(stage);
        }
        let (ack, wait) = oneshot::channel();
        tx.send(Ready::Step {
            slot,
            credit: credit,
            bytes,
            name,
            plan,
            consume_inputs,
            ack,
        })
        .await
        .map_err(|_| closed())?;
        if serial {
            wait.await.map_err(|_| closed())?;
        }
        Ok(())
    }
    async fn run_unretained_pipeline_batch(
        &self,
        batch: &crate::processing_element::compute::EngineBatch,
        group: &Rc<EntityGroup>,
        failed: &Cell<bool>,
    ) -> SimResult {
        let before = self.clock.tick_now().tick();
        let engines = batch
            .alternatives
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        let engine = self.compute_scheduler.acquire_any(&engines).await?;
        let resources = self.block_execution.as_ref().unwrap();
        resources.snapshot.borrow_mut().compute_wait_ticks += self.clock.tick_now().tick() - before;
        running(failed)?;
        let active = resources.stage(Stage::Compute);
        self.block_batch(batch, engine.engine(), group).await?;
        active.complete();
        Ok(())
    }
    async fn run_pipeline_work(
        &self,
        plan: ExecutionPlan,
        group: &Rc<EntityGroup>,
        accumulator: &mut Option<AccumulatorLease>,
        failed: &Cell<bool>,
    ) -> SimResult {
        for stage in plan.stages {
            running(failed)?;
            if stage
                .iter()
                .all(|batch| batch.matrix_accumulator_bytes == 0)
            {
                futures::future::try_join_all(
                    stage
                        .iter()
                        .map(|batch| self.run_unretained_pipeline_batch(batch, group, failed)),
                )
                .await?;
                continue;
            }
            for batch in stage {
                let before = self.clock.tick_now().tick();
                let engines = batch
                    .alternatives
                    .iter()
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>();
                if batch.matrix_accumulator_bytes != 0 {
                    {
                        let mut snapshot =
                            self.block_execution.as_ref().unwrap().snapshot.borrow_mut();
                        snapshot.accumulator_bytes = snapshot
                            .accumulator_bytes
                            .max(batch.matrix_accumulator_bytes);
                    }
                    if accumulator.is_none() {
                        let admission = self
                            .compute_scheduler
                            .acquire_any_with_accumulator(&engines, batch.matrix_accumulator_bytes)
                            .await?;
                        let engine = admission.engine();
                        drop(admission);
                        *accumulator = Some(
                            self.compute_scheduler
                                .retain_accumulator(engine, batch.matrix_accumulator_bytes)
                                .await?,
                        );
                    }
                    let lease = accumulator.as_ref().unwrap();
                    let _engine = self
                        .compute_scheduler
                        .acquire_with_retained_accumulator(lease, &batch)
                        .await?;
                    self.block_execution
                        .as_ref()
                        .unwrap()
                        .snapshot
                        .borrow_mut()
                        .compute_wait_ticks += self.clock.tick_now().tick() - before;
                    running(failed)?;
                    let active = self.block_execution.as_ref().unwrap().stage(Stage::Compute);
                    self.block_batch(&batch, lease.engine(), group).await?;
                    active.complete();
                } else {
                    self.run_unretained_pipeline_batch(&batch, group, failed)
                        .await?;
                }
            }
        }
        Ok(())
    }
}

pub(crate) enum PipelinePlan {
    Matrix(crate::processing_element::program::block_plan::BlockPipelinePlan),
    Program(Box<ProgramPipelinePlan>),
}
impl PipelinePlan {
    pub fn bytes(&self) -> usize {
        match self {
            Self::Matrix(p) => p.storage().sram_bytes,
            Self::Program(p) => p.bytes,
        }
    }
    pub fn validate_capacity(&self, config: &ProgramTaskConfig, capacity: usize) -> SimResult {
        match self {
            Self::Program(p) => p.validate_capacity(config, capacity),
            Self::Matrix(p) if p.storage().sram_bytes > capacity => Err(SimError(format!(
                "{} pipeline requires {} SRAM bytes, available {capacity}",
                config.id,
                p.storage().sram_bytes
            ))),
            Self::Matrix(_) => Ok(()),
        }
    }
}
pub(crate) fn compile(
    config: &ProgramTaskConfig,
    model: &ComputeModel,
    limits: &BlockExecutionConfig,
) -> Result<PipelinePlan, SimError> {
    match crate::processing_element::program::block_plan::plan_block_pipeline(config, model, limits)
    {
        Ok(p) => Ok(PipelinePlan::Matrix(p)),
        Err(_) => plan(config, model, limits).map(|p| PipelinePlan::Program(Box::new(p))),
    }
}
