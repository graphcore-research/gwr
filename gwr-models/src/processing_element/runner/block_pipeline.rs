// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Bounded load/compute/store workers. Custody follows real LSU completion.
use std::cell::Cell;
use std::rc::Rc;

use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use gwr_components::capacity_allocator::CapacityReservation;
use gwr_engine::time::clock::phase;
use gwr_engine::types::{AccessType, SimError, SimResult};

use super::TaskRunner;
use crate::processing_element::activity::ActivityLanes;
use crate::processing_element::block_runtime::{BlockExecutionResources, Stage};
use crate::processing_element::compute::{AccumulatorLease, EngineBatch};
use crate::processing_element::program::block_plan::{
    BlockPipelinePlan, InputChunkPlan, OutputTilePlan,
};
use crate::processing_element::program::pipeline::PipelineMode;
use crate::processing_element::task::ProgramTaskConfig;

pub(super) struct Window {
    pub(super) _reservation: CapacityReservation,
    pub(super) resources: Rc<BlockExecutionResources>,
    pub(super) bytes: usize,
}
impl Drop for Window {
    fn drop(&mut self) {
        let mut s = self.resources.snapshot.borrow_mut();
        s.reserved_window_bytes = s
            .reserved_window_bytes
            .checked_sub(self.bytes)
            .expect("window reservation accounting underflow");
    }
}
pub(super) struct Slot {
    pub(super) id: usize,
    pub(super) generation: u64,
    output: bool,
    window: Rc<Window>,
    returned: mpsc::UnboundedSender<(usize, u64)>,
}
impl Drop for Slot {
    fn drop(&mut self) {
        let mut s = self.window.resources.snapshot.borrow_mut();
        if self.output {
            s.occupied_output_slots = s
                .occupied_output_slots
                .checked_sub(1)
                .expect("output slot underflow");
        } else {
            s.occupied_input_slots = s
                .occupied_input_slots
                .checked_sub(1)
                .expect("input slot underflow");
        }
        let _ = self.returned.unbounded_send((
            self.id,
            self.generation
                .checked_add(1)
                .expect("slot generation overflow"),
        ));
    }
}
pub(super) struct Slots {
    returned: mpsc::UnboundedSender<(usize, u64)>,
    available: mpsc::UnboundedReceiver<(usize, u64)>,
    window: Rc<Window>,
    output: bool,
}
impl Slots {
    pub(super) fn new(count: usize, window: Rc<Window>, output: bool) -> Self {
        let (returned, available) = mpsc::unbounded();
        for id in 0..count {
            returned.unbounded_send((id, 0)).unwrap();
        }
        Self {
            returned,
            available,
            window,
            output,
        }
    }
    pub(super) async fn acquire(&mut self) -> Result<Rc<Slot>, SimError> {
        let (id, generation) = self
            .available
            .next()
            .await
            .ok_or_else(|| SimError("pipeline slot pool closed".into()))?;
        let mut s = self.window.resources.snapshot.borrow_mut();
        if self.output {
            s.occupied_output_slots = s
                .occupied_output_slots
                .checked_add(1)
                .ok_or_else(|| SimError("output slot counter overflow".into()))?;
            s.peak_occupied_output_slots =
                s.peak_occupied_output_slots.max(s.occupied_output_slots);
        } else {
            s.occupied_input_slots = s
                .occupied_input_slots
                .checked_add(1)
                .ok_or_else(|| SimError("input slot counter overflow".into()))?;
            s.peak_occupied_input_slots = s.peak_occupied_input_slots.max(s.occupied_input_slots);
        }
        drop(s);
        Ok(Rc::new(Slot {
            id,
            generation,
            output: self.output,
            window: self.window.clone(),
            returned: self.returned.clone(),
        }))
    }
}
struct ReadyInput {
    chunk: InputChunkPlan,
    slot: Rc<Slot>,
}
struct ReadyOutput {
    tile: OutputTilePlan,
    slot: Rc<Slot>,
}
fn stopped() -> SimError {
    SimError("pipeline stopped after stage failure".into())
}
impl TaskRunner {
    pub(super) async fn run_block_pipeline(
        &self,
        config: &ProgramTaskConfig,
        plan: BlockPipelinePlan,
    ) -> SimResult {
        let r = self
            .block_execution
            .as_ref()
            .ok_or_else(|| SimError("pipeline requires block_execution support".into()))?;
        let d = plan.storage();
        if d.sram_bytes > self.sram_bytes {
            return Err(SimError(format!(
                "{} pipeline requires {} SRAM bytes, available {} ({} input slots, {} output slots)",
                config.id, d.sram_bytes, self.sram_bytes, d.input_slots, d.output_slots
            )));
        }
        let start = self.clock.tick_now().tick();
        let group = self
            .activity_lanes
            .create_group(&format!("{} pipeline window", config.id));
        let waiting = ActivityLanes::begin_in_group(
            &self.activity_lanes.compute_wait,
            &format!("block SRAM wait {}", group.name),
            &group,
        );
        let reservation = self.sram.reserve(d.sram_bytes).await?;
        drop(waiting);
        {
            let mut s = r.snapshot.borrow_mut();
            s.sram_wait_ticks = s
                .sram_wait_ticks
                .checked_add(self.clock.tick_now().tick() - start)
                .ok_or_else(|| SimError("SRAM wait counter overflow".into()))?;
            s.input_buffers = d.input_slots;
            s.output_buffers = d.output_slots;
            s.input_slot_bytes = d.input_slot_bytes;
            s.output_slot_bytes = d.output_slot_bytes;
            s.accumulator_bytes = d.accumulator_bytes;
            s.compute_working_bytes = usize::try_from(d.accumulator_bytes)
                .expect("validated working size")
                + d.scratch_bytes;
            s.reserved_window_bytes = s
                .reserved_window_bytes
                .checked_add(d.sram_bytes)
                .ok_or_else(|| SimError("pipeline window counter overflow".into()))?;
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
            bytes: d.sram_bytes,
        });
        let request = config.pipeline.as_ref().unwrap();
        let mut input_slots = Slots::new(request.prefetch_distance + 1, window.clone(), false);
        let mut output_slots = Slots::new(request.output_buffers, window.clone(), true);
        let failed = Cell::new(false);
        if request.mode == PipelineMode::Serial {
            for tile in plan.output_tiles() {
                let _context = r.compute.reserve(1).await?;
                let engine = tile.input_chunks().next().unwrap().batch().alternatives[0].0;
                let accumulator = self
                    .compute_scheduler
                    .retain_accumulator(engine, tile.accumulator_bytes())
                    .await?;
                for chunk in tile.input_chunks() {
                    let ready = self
                        .load_block(config, chunk, input_slots.acquire().await?, &failed)
                        .await?;
                    self.compute_block(config, &ready, &accumulator, &failed)
                        .await?;
                }
                let ready = self
                    .finalise_block(config, tile, output_slots.acquire().await?, &failed)
                    .await?;
                drop(accumulator);
                drop(_context);
                self.store_block(config, ready, &failed).await?;
            }
            return Ok(());
        }
        let (mut input_tx, mut input_rx) = mpsc::channel::<ReadyInput>(request.input_buffers);
        let (mut output_tx, mut output_rx) = mpsc::channel::<ReadyOutput>(request.output_buffers);
        let load = async {
            let result = async {
                for tile in plan.output_tiles() {
                    for chunk in tile.input_chunks() {
                        if failed.get() {
                            return Err(stopped());
                        }
                        let slot = input_slots.acquire().await?;
                        if failed.get() {
                            return Err(stopped());
                        }
                        let ready = self.load_block(config, chunk, slot, &failed).await?;
                        input_tx.send(ready).await.map_err(|_| stopped())?;
                    }
                }
                Ok(())
            }
            .await;
            if result.is_err() {
                failed.set(true);
            }
            drop(input_tx);
            result
        };
        let compute = async {
            let result = async {
                for tile in plan.output_tiles() {
                    if failed.get() {
                        return Err(stopped());
                    }
                    let _context = r.compute.reserve(1).await?;
                    let engine = tile.input_chunks().next().unwrap().batch().alternatives[0].0;
                    let accumulator = self
                        .compute_scheduler
                        .retain_accumulator(engine, tile.accumulator_bytes())
                        .await?;
                    for index in 0..tile.chunk_count() {
                        let start = self.clock.tick_now().tick();
                        let group = self.activity_lanes.create_group(&format!(
                            "{} output {} chunk {}",
                            config.id,
                            tile.output_tile_id(),
                            index
                        ));
                        let waiting = ActivityLanes::begin_in_group(
                            &self.activity_lanes.compute_wait,
                            &format!("block input readiness wait {}", group.name),
                            &group,
                        );
                        let ready = input_rx.next().await.ok_or_else(stopped)?;
                        drop(waiting);
                        {
                            let mut snapshot = r.snapshot.borrow_mut();
                            snapshot.input_wait_ticks = snapshot
                                .input_wait_ticks
                                .checked_add(self.clock.tick_now().tick() - start)
                                .ok_or_else(|| SimError("pipeline wait counter overflow".into()))?;
                        }
                        if failed.get() {
                            return Err(stopped());
                        }
                        if ready.chunk.output_tile_id() != tile.output_tile_id()
                            || ready.chunk.chunk_index() != index
                        {
                            return Err(SimError("pipeline input generation mismatch".into()));
                        }
                        self.compute_block(config, &ready, &accumulator, &failed)
                            .await?;
                    }
                    let start = self.clock.tick_now().tick();
                    let group = self.activity_lanes.create_group(&format!(
                        "{} output {}",
                        config.id,
                        tile.output_tile_id()
                    ));
                    let waiting = ActivityLanes::begin_in_group(
                        &self.activity_lanes.compute_wait,
                        &format!("block output slot wait {}", group.name),
                        &group,
                    );
                    let slot = output_slots.acquire().await?;
                    drop(waiting);
                    {
                        let mut snapshot = r.snapshot.borrow_mut();
                        snapshot.output_wait_ticks = snapshot
                            .output_wait_ticks
                            .checked_add(self.clock.tick_now().tick() - start)
                            .ok_or_else(|| SimError("pipeline wait counter overflow".into()))?;
                    }
                    if failed.get() {
                        return Err(stopped());
                    }
                    let ready = self.finalise_block(config, tile, slot, &failed).await?;
                    drop(accumulator);
                    drop(_context);
                    output_tx.send(ready).await.map_err(|_| stopped())?;
                }
                Ok(())
            }
            .await;
            if result.is_err() {
                failed.set(true);
            }
            drop(input_rx);
            drop(output_tx);
            result
        };
        let store = async {
            let result = async {
                while let Some(ready) = output_rx.next().await {
                    if failed.get() {
                        return Err(stopped());
                    }
                    self.store_block(config, ready, &failed).await?;
                }
                Ok(())
            }
            .await;
            if result.is_err() {
                failed.set(true);
            }
            drop(output_rx);
            result
        };
        // Join rather than try_join: issued requests must settle before
        // returning.
        let (load, compute, store) = futures::join!(load, compute, store);
        let errors = [load, compute, store]
            .into_iter()
            .filter_map(Result::err)
            .collect::<Vec<_>>();
        if let Some(error) = errors.iter().find(|error| error.0 != stopped().0) {
            return Err(SimError(error.0.clone()));
        }
        if let Some(error) = errors.into_iter().next() {
            return Err(error);
        }
        Ok(())
    }
    async fn load_block(
        &self,
        config: &ProgramTaskConfig,
        chunk: InputChunkPlan,
        slot: Rc<Slot>,
        stopped_flag: &Cell<bool>,
    ) -> Result<ReadyInput, SimError> {
        let r = self.block_execution.as_ref().unwrap();
        let permit = r.load.reserve(1).await?;
        if stopped_flag.get() {
            return Err(stopped());
        }
        let stage = Rc::new(r.stage(Stage::Load));
        let group = self.activity_lanes.create_group(&format!(
            "{} output {} chunk {} input slot {} generation {}",
            config.id,
            chunk.output_tile_id(),
            chunk.chunk_index(),
            slot.id,
            slot.generation
        ));
        let custody: Rc<dyn std::any::Any> = Rc::new((slot.clone(), permit, stage.clone()));
        for view in chunk.operands() {
            if stopped_flag.get() {
                return Err(stopped());
            }
            self.lsu
                .access_ranges_retaining(
                    AccessType::ReadRequest,
                    view.address_ranges(),
                    &self.activity_lanes.lsu_read,
                    &format!("block load {}", group.name),
                    &group,
                    custody.clone(),
                )
                .await?;
            self.record_read_bytes(view.layout().num_access_bytes())?;
            if stopped_flag.get() {
                return Err(stopped());
            }
            r.stage_local(true, view.num_packed_bytes()).await?;
        }
        stage.complete();
        Ok(ReadyInput { chunk, slot })
    }
    async fn compute_block(
        &self,
        config: &ProgramTaskConfig,
        ready: &ReadyInput,
        accumulator: &AccumulatorLease,
        stopped_flag: &Cell<bool>,
    ) -> SimResult {
        let r = self.block_execution.as_ref().unwrap();
        let group = self.activity_lanes.create_group(&format!(
            "{} output {} chunk {} input slot {} generation {}",
            config.id,
            ready.chunk.output_tile_id(),
            ready.chunk.chunk_index(),
            ready.slot.id,
            ready.slot.generation
        ));
        let bytes = ready
            .chunk
            .operands()
            .iter()
            .map(|v| v.num_packed_bytes())
            .sum();
        r.stage_local(false, bytes).await?;
        if stopped_flag.get() {
            return Err(stopped());
        }
        let start = self.clock.tick_now().tick();
        let waiting = ActivityLanes::begin_in_group(
            &self.activity_lanes.compute_wait,
            &format!("block engine wait {}", group.name),
            &group,
        );
        let _engine = self
            .compute_scheduler
            .acquire_with_retained_accumulator(accumulator, ready.chunk.batch())
            .await?;
        drop(waiting);
        {
            let mut snapshot = r.snapshot.borrow_mut();
            snapshot.compute_wait_ticks = snapshot
                .compute_wait_ticks
                .checked_add(self.clock.tick_now().tick() - start)
                .ok_or_else(|| SimError("pipeline wait counter overflow".into()))?;
        }
        if stopped_flag.get() {
            return Err(stopped());
        }
        let stage = r.stage(Stage::Compute);
        self.block_batch(ready.chunk.batch(), accumulator.engine(), &group)
            .await?;
        stage.complete();
        Ok(())
    }
    pub(super) async fn block_batch(
        &self,
        batch: &EngineBatch,
        engine: crate::processing_element::compute::EngineId,
        group: &Rc<gwr_track::entity::EntityGroup>,
    ) -> SimResult {
        let ticks = batch
            .alternatives
            .iter()
            .find(|(id, _)| *id == engine)
            .ok_or_else(|| SimError("pipeline engine is not eligible".into()))?
            .1;
        self.clock.wait_phase(phase::END).await;
        let _activity = ActivityLanes::begin_in_group(
            &self.activity_lanes.compute[engine.0],
            &format!("block compute {}", group.name),
            group,
        );
        if let Some(monitor) = &self.flop_monitor {
            monitor.record_interval(ticks, batch.machine_ops.flop_equivalents() as f64);
        }
        self.clock.wait_ticks(ticks)?.await;
        let mut stats = self.stats.borrow_mut();
        stats.machine_ops = stats
            .machine_ops
            .clone()
            .checked_add(batch.machine_ops.clone())?;
        Ok(())
    }
    async fn finalise_block(
        &self,
        config: &ProgramTaskConfig,
        tile: OutputTilePlan,
        slot: Rc<Slot>,
        stopped_flag: &Cell<bool>,
    ) -> Result<ReadyOutput, SimError> {
        let r = self.block_execution.as_ref().unwrap();
        let stage = r.stage(Stage::Finalise);
        let group = self.activity_lanes.create_group(&format!(
            "{} output {} finalise slot {} generation {}",
            config.id,
            tile.output_tile_id(),
            slot.id,
            slot.generation
        ));
        r.stage_local(false, tile.accumulator_bytes() as usize)
            .await?;
        for batches in &tile.finalisation().stages {
            for batch in batches {
                if stopped_flag.get() {
                    return Err(stopped());
                }
                let ids = batch
                    .alternatives
                    .iter()
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>();
                let reservation = self
                    .compute_scheduler
                    .acquire_any_with_accumulator(&ids, batch.matrix_accumulator_bytes)
                    .await?;
                self.block_batch(batch, reservation.engine(), &group)
                    .await?;
            }
        }
        r.stage_local(true, tile.output_view().num_packed_bytes())
            .await?;
        stage.complete();
        Ok(ReadyOutput { tile, slot })
    }
    async fn store_block(
        &self,
        config: &ProgramTaskConfig,
        ready: ReadyOutput,
        stopped_flag: &Cell<bool>,
    ) -> SimResult {
        let r = self.block_execution.as_ref().unwrap();
        let permit = r.store.reserve(1).await?;
        if stopped_flag.get() {
            return Err(stopped());
        }
        let stage = Rc::new(r.stage(Stage::Store));
        let group = self.activity_lanes.create_group(&format!(
            "{} output {} store slot {} generation {}",
            config.id,
            ready.tile.output_tile_id(),
            ready.slot.id,
            ready.slot.generation
        ));
        let custody: Rc<dyn std::any::Any> = Rc::new((ready.slot.clone(), permit, stage.clone()));
        r.stage_local(false, ready.tile.output_view().num_packed_bytes())
            .await?;
        if stopped_flag.get() {
            return Err(stopped());
        }
        self.lsu
            .access_ranges_retaining(
                AccessType::WriteNonPostedRequest,
                ready.tile.output_view().address_ranges(),
                &self.activity_lanes.lsu_write,
                &format!("block store {}", group.name),
                &group,
                custody,
            )
            .await?;
        self.record_write(ready.tile.output_view())?;
        stage.complete();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt;
    use futures::executor::block_on;
    use gwr_components::capacity_allocator::CapacityAllocator;
    use gwr_engine::engine::Engine;

    use super::*;
    fn resources(engine: &mut Engine) -> Rc<BlockExecutionResources> {
        let config=yaml_serde::from_str("max_compute_tiles: 1\nmax_load_chunks: 1\nmax_store_tiles: 1\ntransfer_port: lsu\naccumulator_storage: shared_sram").unwrap();
        BlockExecutionResources::new(config, &engine.default_clock(), engine.top()).unwrap()
    }
    #[test]
    fn cancelled_slot_wait_preserves_generation_and_window_custody() {
        let mut engine = Engine::default();
        let resources = resources(&mut engine);
        let allocator = CapacityAllocator::new(engine.top(), "test_window", 512, "bytes").unwrap();
        block_on(async {
            let reservation = allocator.reserve(512).await.unwrap();
            resources.snapshot.borrow_mut().reserved_window_bytes = 512;
            let window = Rc::new(Window {
                _reservation: reservation,
                resources: resources.clone(),
                bytes: 512,
            });
            let mut slots = Slots::new(1, window.clone(), false);
            let first = slots.acquire().await.unwrap();
            assert_eq!((first.id, first.generation), (0, 0));
            assert!(slots.acquire().now_or_never().is_none());
            let packet_custody = first.clone();
            drop(first);
            assert!(slots.acquire().now_or_never().is_none());
            drop(packet_custody);
            let next = slots.acquire().await.unwrap();
            assert_eq!((next.id, next.generation), (0, 1));
            drop(window);
            drop(slots);
            assert_eq!(allocator.used(), 512);
            drop(next);
            assert_eq!(allocator.used(), 0);
            assert_eq!(resources.snapshot.borrow().occupied_input_slots, 0);
        });
    }
    #[test]
    fn packet_owner_retains_transfer_permit_and_slot_after_parent_drop() {
        let mut engine = Engine::default();
        let resources = resources(&mut engine);
        let allocator = CapacityAllocator::new(engine.top(), "test_window", 512, "bytes").unwrap();
        block_on(async {
            let reservation = allocator.reserve(512).await.unwrap();
            resources.snapshot.borrow_mut().reserved_window_bytes = 512;
            let window = Rc::new(Window {
                _reservation: reservation,
                resources: resources.clone(),
                bytes: 512,
            });
            let mut slots = Slots::new(1, window.clone(), true);
            let slot = slots.acquire().await.unwrap();
            let permit = resources.store.reserve(1).await.unwrap();
            let stage = Rc::new(resources.stage(Stage::Store));
            let custody = Rc::new((slot, permit, stage));
            let packet = custody.clone();
            drop(custody);
            drop(slots);
            drop(window);
            assert_eq!(resources.store.used(), 1);
            assert_eq!(resources.snapshot.borrow().active_store_tiles, 1);
            assert_eq!(allocator.used(), 512);
            drop(packet);
            assert_eq!(resources.store.used(), 0);
            assert_eq!(allocator.used(), 0);
            assert_eq!(resources.snapshot.borrow().cancelled_store_tiles, 1);
        });
    }
    #[test]
    fn failure_while_load_waits_prevents_new_operand_issue() {
        use super::super::block_pipeline_tests::{fixture, program};
        for blocked_permit in [true, false] {
            let mut engine = Engine::default();
            let (runner, _done) = fixture(&mut engine, 30);
            let inspect = runner.clone();
            let r = runner.block_execution.as_ref().unwrap().clone();
            let held = if blocked_permit {
                Some(block_on(r.load.reserve(1)).unwrap())
            } else {
                None
            };
            let failed = Rc::new(Cell::new(false));
            let stop = failed.clone();
            let clock = runner.clock.clone();
            engine.spawner().spawn(async move {
                let config = program(2, 1, 1);
                let plan = crate::processing_element::program::block_plan::plan_block_pipeline(
                    &config,
                    &runner.compute_model,
                    &r.config,
                )?;
                let bytes = plan.storage().sram_bytes;
                let reservation = runner.sram.reserve(bytes).await?;
                r.snapshot.borrow_mut().reserved_window_bytes = bytes;
                let window = Rc::new(Window {
                    _reservation: reservation,
                    resources: r.clone(),
                    bytes,
                });
                let mut slots = Slots::new(2, window, false);
                let chunk = plan
                    .output_tiles()
                    .next()
                    .unwrap()
                    .input_chunks()
                    .next()
                    .unwrap();
                assert!(
                    runner
                        .load_block(&config, chunk, slots.acquire().await?, &failed)
                        .await
                        .is_err()
                );
                Ok(())
            });
            engine.spawner().spawn(async move {
                clock.wait_ticks(10)?.await;
                stop.set(true);
                drop(held);
                Ok(())
            });
            engine.run().unwrap();
            // With no admission wait, only A was already issued. B must never
            // issue.
            assert_eq!(
                inspect.stats.borrow().read_bytes,
                if blocked_permit { 0 } else { 32 }
            );
            assert_eq!(inspect.sram.used(), 0);
            assert_eq!(inspect.block_execution.as_ref().unwrap().load.used(), 0);
        }
    }
}
