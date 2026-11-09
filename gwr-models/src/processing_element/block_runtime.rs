// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! PE-wide admission and observations shared by all block pipeline invocations.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gwr_components::capacity_allocator::CapacityAllocator;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::{SimError, SimResult};
use gwr_track::entity::Entity;

use super::block_execution::BlockExecutionConfig;
use super::boundary_payload::{BoundaryPayloadDirection, BoundaryPayloadService};

/// Aggregate observations; local byte counts are logical staging, not HBM
/// traffic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockExecutionSnapshot {
    pub completed_load_chunks: u64,
    pub completed_compute_chunks: u64,
    pub completed_finalisations: u64,
    pub completed_store_tiles: u64,
    pub cancelled_load_chunks: u64,
    pub cancelled_compute_chunks: u64,
    pub cancelled_finalisations: u64,
    pub cancelled_store_tiles: u64,
    pub active_load_chunks: usize,
    pub active_compute_chunks: usize,
    pub active_finalisations: usize,
    pub active_store_tiles: usize,
    pub peak_load_chunks: usize,
    pub peak_compute_chunks: usize,
    pub peak_finalisations: usize,
    pub peak_store_tiles: usize,
    pub input_buffers: usize,
    pub output_buffers: usize,
    pub input_slot_bytes: usize,
    pub output_slot_bytes: usize,
    pub reserved_window_bytes: usize,
    pub peak_reserved_window_bytes: usize,
    pub accumulator_bytes: u64,
    /// Complete generic working arena, including retained scalar state.
    pub compute_working_bytes: usize,
    pub occupied_input_slots: usize,
    pub peak_occupied_input_slots: usize,
    pub occupied_output_slots: usize,
    pub peak_occupied_output_slots: usize,
    pub local_read_bytes: usize,
    pub local_write_bytes: usize,
    pub local_service_modelled: bool,
    pub sram_wait_ticks: u64,
    pub input_wait_ticks: u64,
    pub output_wait_ticks: u64,
    pub compute_wait_ticks: u64,
    /// Elapsed ticks with at least one active load and compute stage.
    pub load_compute_overlap_ticks: u64,
    /// Elapsed ticks with at least one active store and compute stage.
    pub store_compute_overlap_ticks: u64,
}

pub(crate) struct BlockExecutionResources {
    pub config: BlockExecutionConfig,
    pub load: CapacityAllocator,
    pub compute: CapacityAllocator,
    pub store: CapacityAllocator,
    pub snapshot: RefCell<BlockExecutionSnapshot>,
    local: Option<Rc<BoundaryPayloadService>>,
    clock: Clock,
    observed_tick: Cell<u64>,
}

impl BlockExecutionResources {
    pub fn new(
        config: BlockExecutionConfig,
        clock: &Clock,
        entity: &Rc<Entity>,
    ) -> Result<Rc<Self>, SimError> {
        config.validate()?;
        let local = config
            .local_memory_service
            .as_ref()
            .map(|s| BoundaryPayloadService::new(s.bytes_per_tick, clock))
            .transpose()?;
        Ok(Rc::new(Self {
            snapshot: RefCell::new(BlockExecutionSnapshot {
                local_service_modelled: local.is_some(),
                ..Default::default()
            }),
            config,
            load: CapacityAllocator::new(entity, "block_load", 1, "jobs")?,
            compute: CapacityAllocator::new(entity, "block_compute", 1, "tiles")?,
            store: CapacityAllocator::new(entity, "block_store", 1, "jobs")?,
            local,
            clock: clock.clone(),
            observed_tick: Cell::new(clock.tick_now().tick()),
        }))
    }

    pub async fn stage_local(&self, write: bool, bytes: usize) -> SimResult {
        {
            let mut snapshot = self.snapshot.borrow_mut();
            let counter = if write {
                &mut snapshot.local_write_bytes
            } else {
                &mut snapshot.local_read_bytes
            };
            *counter = counter
                .checked_add(bytes)
                .ok_or_else(|| SimError("pipeline local bytes overflow".into()))?;
        }
        if let Some(service) = &self.local {
            service
                .reserve(
                    if write {
                        BoundaryPayloadDirection::Write
                    } else {
                        BoundaryPayloadDirection::Read
                    },
                    bytes,
                )?
                .complete()
                .await?;
        }
        Ok(())
    }

    /// Observe intersections through the current tick, even during live stages.
    pub fn snapshot(&self) -> BlockExecutionSnapshot {
        self.flush_overlap();
        self.snapshot.borrow().clone()
    }

    fn flush_overlap(&self) {
        let now = self.clock.tick_now().tick();
        let elapsed = now
            .checked_sub(self.observed_tick.replace(now))
            .expect("pipeline clock moved backwards");
        let mut snapshot = self.snapshot.borrow_mut();
        if snapshot.active_compute_chunks != 0 {
            if snapshot.active_load_chunks != 0 {
                snapshot.load_compute_overlap_ticks = snapshot
                    .load_compute_overlap_ticks
                    .checked_add(elapsed)
                    .expect("pipeline load/compute overlap counter overflow");
            }
            if snapshot.active_store_tiles != 0 {
                snapshot.store_compute_overlap_ticks = snapshot
                    .store_compute_overlap_ticks
                    .checked_add(elapsed)
                    .expect("pipeline store/compute overlap counter overflow");
            }
        }
    }

    pub fn stage(self: &Rc<Self>, kind: Stage) -> StageGuard {
        self.flush_overlap();
        let mut snapshot = self.snapshot.borrow_mut();
        let (active, peak) = match kind {
            Stage::Load => {
                let s = &mut *snapshot;
                (&mut s.active_load_chunks, &mut s.peak_load_chunks)
            }
            Stage::Compute => {
                let s = &mut *snapshot;
                (&mut s.active_compute_chunks, &mut s.peak_compute_chunks)
            }
            Stage::Finalise => {
                let s = &mut *snapshot;
                (&mut s.active_finalisations, &mut s.peak_finalisations)
            }
            Stage::Store => {
                let s = &mut *snapshot;
                (&mut s.active_store_tiles, &mut s.peak_store_tiles)
            }
        };
        *active = active
            .checked_add(1)
            .expect("pipeline active stage counter overflow");
        *peak = (*peak).max(*active);
        StageGuard {
            resources: self.clone(),
            kind,
            complete: Cell::new(false),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Stage {
    Load,
    Compute,
    Finalise,
    Store,
}

pub(crate) struct StageGuard {
    resources: Rc<BlockExecutionResources>,
    kind: Stage,
    complete: Cell<bool>,
}
impl StageGuard {
    pub fn complete(&self) {
        self.complete.set(true);
    }
}
impl Drop for StageGuard {
    fn drop(&mut self) {
        self.resources.flush_overlap();
        let mut snapshot = self.resources.snapshot.borrow_mut();
        let s = &mut *snapshot;
        let (active, completed, cancelled) = match self.kind {
            Stage::Load => (
                &mut s.active_load_chunks,
                &mut s.completed_load_chunks,
                &mut s.cancelled_load_chunks,
            ),
            Stage::Compute => (
                &mut s.active_compute_chunks,
                &mut s.completed_compute_chunks,
                &mut s.cancelled_compute_chunks,
            ),
            Stage::Finalise => (
                &mut s.active_finalisations,
                &mut s.completed_finalisations,
                &mut s.cancelled_finalisations,
            ),
            Stage::Store => (
                &mut s.active_store_tiles,
                &mut s.completed_store_tiles,
                &mut s.cancelled_store_tiles,
            ),
        };
        *active = active
            .checked_sub(1)
            .expect("pipeline active stage counter underflow");
        let counter = if self.complete.get() {
            completed
        } else {
            cancelled
        };
        *counter = counter
            .checked_add(1)
            .expect("pipeline completed/cancelled stage counter overflow");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_snapshots_measure_intersections_and_flush_live_intervals() {
        let mut engine = gwr_engine::engine::Engine::default();
        let clock = engine.default_clock();
        let config = yaml_serde::from_str("max_compute_tiles: 1\nmax_load_chunks: 1\nmax_store_tiles: 1\ntransfer_port: lsu\naccumulator_storage: shared_sram").unwrap();
        let resources = BlockExecutionResources::new(config, &clock, engine.top()).unwrap();
        engine.spawn(async move {
            let load = resources.stage(Stage::Load);
            clock.wait_ticks(2)?.await;
            let compute = resources.stage(Stage::Compute);
            clock.wait_ticks(3)?.await;
            let snapshot = resources.snapshot();
            assert_eq!(snapshot.load_compute_overlap_ticks, 3);
            assert_eq!(snapshot.store_compute_overlap_ticks, 0);
            assert_eq!(snapshot.active_load_chunks, 1);
            load.complete();
            drop(load);
            clock.wait_ticks(2)?.await;
            let store = resources.stage(Stage::Store);
            clock.wait_ticks(4)?.await;
            assert_eq!(resources.snapshot().store_compute_overlap_ticks, 4);
            drop(compute);
            drop(store);
            let finalise = resources.stage(Stage::Finalise);
            assert_eq!(resources.snapshot().active_finalisations, 1);
            finalise.complete();
            drop(finalise);
            let snapshot = resources.snapshot();
            assert_eq!(snapshot.load_compute_overlap_ticks, 3);
            assert_eq!(snapshot.store_compute_overlap_ticks, 4);
            assert_eq!(snapshot.completed_load_chunks, 1);
            assert_eq!(snapshot.cancelled_compute_chunks, 1);
            assert_eq!(snapshot.cancelled_store_tiles, 1);
            assert_eq!(snapshot.completed_finalisations, 1);
            assert_eq!(snapshot.active_finalisations, 0);
            Ok(())
        });
        engine.run().unwrap();
    }
}

#[cfg(test)]
mod counter_tests {
    use super::*;

    #[test]
    fn local_byte_counter_overflow_rejects_without_wrapping() {
        let mut engine = gwr_engine::engine::Engine::default();
        let clock = engine.default_clock();
        let config = yaml_serde::from_str("max_compute_tiles: 1\nmax_load_chunks: 1\nmax_store_tiles: 1\ntransfer_port: lsu\naccumulator_storage: shared_sram").unwrap();
        let resources = BlockExecutionResources::new(config, &clock, engine.top()).unwrap();
        engine.spawn(async move {
            resources.stage_local(true, usize::MAX).await?;
            assert!(resources.stage_local(true, 1).await.is_err());
            assert_eq!(resources.snapshot().local_write_bytes, usize::MAX);
            assert_eq!(resources.snapshot().local_read_bytes, 0);
            Ok(())
        });
        engine.run().unwrap();
    }
}
