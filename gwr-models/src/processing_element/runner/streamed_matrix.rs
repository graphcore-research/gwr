// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Exact source vector multiplicity and persistent local Split-K storage.
//! This models traffic/lifetimes/service, not numerical MFMA or BF16 values.
use std::cell::RefCell;
use std::rc::Rc;

use gwr_components::capacity_allocator::{CapacityAllocator, CapacityReservation};
use gwr_engine::types::{SimError, SimResult};

use crate::processing_element::compute::{AccumulatorLease, ComputeScheduler, EngineId};
use crate::processing_element::tensors::dtype::DataType;
use crate::processing_element::tensors::{HasShape, TensorView};
fn error(s: &str) -> SimError {
    SimError(s.into())
}
pub const LDS_BYTES: usize = 49152;
pub const ACCUMULATOR_BYTES: u64 = 8192;
pub const SCRATCH_BYTES: usize = 4096;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceOperand {
    A,
    B,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceVectorIssue {
    pub operand: SourceOperand,
    pub chunk: usize,
    pub register: usize,
    pub thread: usize,
    pub source_row: usize,
    pub k_column: usize,
    pub address: u64,
}
impl SourceVectorIssue {
    pub fn range(self) -> std::ops::Range<u128> {
        u128::from(self.address)..u128::from(self.address) + 16
    }
}
#[derive(Clone, Debug)]
pub struct StreamedMatrixTaskConfig {
    pub id: String,
    pub a: TensorView,
    pub b: TensorView,
    pub c: TensorView,
    pub invocation: u64,
    pub generation: u64,
    pub m: usize,
    pub n: usize,
    pub k: usize,
    pub split: usize,
    pub participant: usize,
    pub n_offset: usize,
    pub execution: crate::processing_element::compute::ExecutionMode,
}
impl StreamedMatrixTaskConfig {
    pub fn validate(&self) -> SimResult {
        if self.id.is_empty()
            || !matches!(
                (self.m, self.n, self.k, self.split),
                (1, 64, 512, 2) | (1 | 2 | 4, 7168, 2048, 4)
            )
            || self.participant >= self.split
            || !self.n_offset.is_multiple_of(64)
            || self.n_offset.checked_add(64).is_none_or(|end| end > self.n)
        {
            return Err(error("unsupported source Split-K geometry/participant"));
        }
        if !self.a.is_full_view()
            || !self.b.is_full_view()
            || self.a.dtype() != &DataType::Bf16
            || self.b.dtype() != &DataType::Bf16
            || self.a.shape().dims() != [self.m, self.k]
            || self.b.shape().dims() != [self.n, self.k]
            || !self.a.tensor().addr().is_multiple_of(16)
            || !self.b.tensor().addr().is_multiple_of(16)
        {
            return Err(error(
                "source requires original full contiguous BF16 A[M,K], B[N,K] with 16B bases",
            ));
        }
        if self.execution.accumulator != DataType::Fp32
            || self.execution.sparsity != crate::processing_element::compute::Sparsity::Dense
            || self.execution.instruction.trim().is_empty()
        {
            return Err(error(
                "source requires explicit dense BF16→FP32 matrix execution",
            ));
        }
        if self.c.dtype() != &DataType::Bf16
            || !self.c.is_full_view()
            || self.c.shape().dims() != [self.m, self.n]
            || !self.c.tensor().addr().is_multiple_of(4)
            || self.invocation == 0
            || self.generation == 0
        {
            return Err(error(
                "source requires exact full BF16 C[M,N] and live invocation/generation",
            ));
        }
        if self.c.first_overlapping_byte_ranges(&self.a).is_some()
            || self.c.first_overlapping_byte_ranges(&self.b).is_some()
        {
            return Err(error("source output cannot overlap ordinary input backing"));
        }
        if self.chunk_count() < 2 {
            return Err(error("source requires nonempty K128 hot loop"));
        }
        Ok(())
    }
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.k.checked_div(self.split).map_or(0, |k| k / 128)
    }
    pub fn issues(&self, chunk: usize) -> Result<SourceIssuePlan, SimError> {
        self.validate()?;
        if chunk >= self.chunk_count() {
            return Err(error("source chunk out of range"));
        }
        Ok(SourceIssuePlan {
            config: self.clone(),
            chunk,
            index: 0,
        })
    }
}
/// B before A, register-major/thread-major original issues. No union/dedup.
/// XOR16 is a source address permutation; padded A rows use source row zero.
pub struct SourceIssuePlan {
    config: StreamedMatrixTaskConfig,
    chunk: usize,
    index: usize,
}
impl Iterator for SourceIssuePlan {
    type Item = SourceVectorIssue;
    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= 1536 {
            return None;
        }
        let index = self.index;
        self.index += 1;
        let (operand, local) = if index < 1024 {
            (SourceOperand::B, index)
        } else {
            (SourceOperand::A, index - 1024)
        };
        let register = local / 128;
        let thread = local % 128;
        let global_tid = register * 128 + thread;
        let row = global_tid / 16;
        let column_bytes = (global_tid % 16 * 16) ^ ((row % 16) * 16);
        let k_column = self.config.participant * (self.config.k / self.config.split)
            + self.chunk * 128
            + column_bytes / 2;
        let (source_row, base) = match operand {
            SourceOperand::B => (self.config.n_offset + row, self.config.b.tensor().addr()),
            SourceOperand::A => (
                if row < self.config.m { row } else { 0 },
                self.config.a.tensor().addr(),
            ),
        };
        // Full validated parent tensors guarantee the complete 16B vector
        // bounds.
        let offset = (source_row * self.config.k + k_column) * 2;
        Some(SourceVectorIssue {
            operand,
            chunk: self.chunk,
            register,
            thread,
            source_row,
            k_column,
            address: base + offset as u64,
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = 1536 - self.index;
        (n, Some(n))
    }
}
impl ExactSizeIterator for SourceIssuePlan {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    LdsRing,
    Scratch,
}
struct Storage {
    _lds: CapacityReservation,
    accumulator: RefCell<Option<AccumulatorLease>>,
    phase: RefCell<Phase>,
    completed_chunks: RefCell<usize>,
    chunks: usize,
    generation: u64,
}
/// Separate LDS and engine-local accumulator. No global partial allocation.
pub struct StreamStorageLease {
    inner: Rc<Storage>,
}
impl StreamStorageLease {
    pub async fn acquire(
        sram: &CapacityAllocator,
        scheduler: &ComputeScheduler,
        engine: EngineId,
        chunks: usize,
    ) -> Result<Self, SimError> {
        if !matches!(chunks, 2 | 4) || sram.capacity() < LDS_BYTES {
            return Err(error("stream LDS/chunk admission failed"));
        }
        // Match ordinary runner lock order: local storage before compute
        // storage. Inverting this order can deadlock an ordinary tile
        // holding SRAM while waiting for an accumulator against a
        // stream holding that accumulator.
        let lds = sram.reserve(LDS_BYTES).await?;
        let accumulator = scheduler
            .retain_accumulator(engine, ACCUMULATOR_BYTES)
            .await?;
        Ok(Self {
            inner: Rc::new(Storage {
                _lds: lds,
                accumulator: RefCell::new(Some(accumulator)),
                phase: RefCell::new(Phase::LdsRing),
                completed_chunks: RefCell::new(0),
                chunks,
                generation: 1,
            }),
        })
    }
    pub(super) fn accumulator(&self) -> std::cell::Ref<'_, AccumulatorLease> {
        std::cell::Ref::map(self.inner.accumulator.borrow(), |x| {
            x.as_ref()
                .expect("scratch phase has no compute accumulator")
        })
    }
    pub(super) fn complete_chunk(&self, chunk: usize) -> SimResult {
        if *self.inner.phase.borrow() != Phase::LdsRing
            || *self.inner.completed_chunks.borrow() != chunk
            || chunk >= self.inner.chunks
        {
            return Err(error("stream chunk/barrier order mismatch"));
        }
        *self.inner.completed_chunks.borrow_mut() += 1;
        Ok(())
    }
    /// Called only by runner after final compute and the full padded FP32→BF16
    /// truncation service barrier. The same LDS backing becomes scratch,
    /// retaining the LDS allocation until the last operand handle's real
    /// output ACK.
    pub(super) fn alias_after_truncation(self) -> Result<ScratchLease, SimError> {
        if *self.inner.phase.borrow() != Phase::LdsRing
            || *self.inner.completed_chunks.borrow() != self.inner.chunks
        {
            return Err(error("scratch alias before last compute barrier"));
        }
        *self.inner.phase.borrow_mut() = Phase::Scratch;
        self.inner.accumulator.borrow_mut().take();
        Ok(ScratchLease { inner: self.inner })
    }
}
pub struct ScratchLease {
    inner: Rc<Storage>,
}
/// Private construction preserves exact scratch generation and aligned pair.
/// The runner retains this handle through actual atomic operand issue and ACK.
pub struct LocalOperandHandle {
    inner: Rc<Storage>,
    generation: u64,
    pair_offset: usize,
}
impl ScratchLease {
    pub(super) fn pair(&self, pair_offset: usize) -> Result<LocalOperandHandle, SimError> {
        if !pair_offset.is_multiple_of(4)
            || pair_offset
                .checked_add(4)
                .is_none_or(|end| end > SCRATCH_BYTES)
            || *self.inner.phase.borrow() != Phase::Scratch
        {
            return Err(error("invalid local BF16 scratch pair"));
        }
        Ok(LocalOperandHandle {
            inner: self.inner.clone(),
            generation: self.inner.generation,
            pair_offset,
        })
    }
}
impl LocalOperandHandle {
    pub fn byte_offset(&self) -> Result<usize, SimError> {
        if self.generation != self.inner.generation || *self.inner.phase.borrow() != Phase::Scratch
        {
            return Err(error("stale scratch generation"));
        }
        Ok(self.pair_offset)
    }
}

impl super::TaskRunner {
    pub(super) fn preflight_stream(
        &self,
        config: &StreamedMatrixTaskConfig,
    ) -> Result<
        (
            crate::processing_element::compute::EngineBatch,
            crate::processing_element::compute::ExecutionPlan,
        ),
        SimError,
    > {
        use crate::processing_element::compute::{ComputeWork, MatrixWorkShape};
        config.validate()?;
        if self.sram_bytes < LDS_BYTES {
            return Err(error(
                "source LDS requires 49152B separate from accumulator",
            ));
        }
        for chunk in 0..config.chunk_count() {
            self.lsu.validate_source_issues(config.issues(chunk)?)?;
        }
        let work = ComputeWork::matrix_mac(
            vec![DataType::Bf16, DataType::Bf16],
            MatrixWorkShape {
                m: 32,
                n: 64,
                k: 128,
            },
        )?
        .with_execution(Some(config.execution.clone()));
        let plan = self.compute_model.lower(&work)?;
        if plan.stages.len() != 1 || plan.stages[0].len() != 1 {
            return Err(error(
                "source matrix chunk must lower to one retained engine batch",
            ));
        }
        let batch = plan.stages[0][0].clone();
        let truncate = self.compute_model.lower(&ComputeWork::operation(
            "convert",
            vec![DataType::Fp32],
            2048,
        ))?;
        Ok((batch, truncate))
    }
    /// Actual LSU chunk issues overlap current compute using two retained LDS
    /// stages. Engine occupancy is chunk-local; accumulator survives every
    /// load. No global partial output or matrix output cast is synthesized.
    pub(super) async fn run_stream(
        &self,
        config: &StreamedMatrixTaskConfig,
    ) -> Result<ScratchLease, SimError> {
        let (batch, truncate) = self.preflight_stream(config)?;
        let group = self.activity_lanes.create_group(&config.id);
        let storage = StreamStorageLease::acquire(
            &self.sram,
            &self.compute_scheduler,
            batch.engine,
            config.chunk_count(),
        )
        .await?;
        {
            let mut stats = self.stats.borrow_mut();
            stats.peak_sram_bytes = stats.peak_sram_bytes.max(self.sram.used());
        }
        self.lsu
            .access_source_issues(
                config.issues(0)?,
                &self.activity_lanes.lsu_read,
                &format!("{} chunk0", config.id),
                &group,
                storage.inner.clone(),
                self.stats.clone(),
            )
            .await?;
        for chunk in 0..config.chunk_count() {
            let compute = self.run_retained_chunk(&config.id, &batch, &storage, &group);
            if chunk + 1 < config.chunk_count() {
                let loads = async {
                    self.lsu
                        .access_source_issues(
                            config.issues(chunk + 1)?,
                            &self.activity_lanes.lsu_read,
                            &format!("{} chunk{}", config.id, chunk + 1),
                            &group,
                            storage.inner.clone(),
                            self.stats.clone(),
                        )
                        .await?;
                    Ok::<(), SimError>(())
                };
                futures::try_join!(compute, loads)?;
            } else {
                compute.await?;
            }
            storage.complete_chunk(chunk)?;
        }
        // Source gpu.barrier: all source loads and last MFMA completed above.
        // The padded 2048 truncations occur once, without external SRAM
        // staging traffic.
        for stage in truncate.stages {
            for batch in stage {
                self.run_stream_scalar_batch(&config.id, batch, &group)
                    .await?;
            }
        }
        storage.alias_after_truncation()
    }
    async fn run_retained_chunk(
        &self,
        id: &str,
        batch: &crate::processing_element::compute::EngineBatch,
        storage: &StreamStorageLease,
        group: &Rc<gwr_track::entity::EntityGroup>,
    ) -> SimResult {
        use crate::processing_element::activity::ActivityLanes;
        let acquire = {
            let lease = storage.accumulator();
            self.compute_scheduler
                .acquire_with_retained_accumulator(&lease, batch)
        };
        let reservation = acquire.await?;
        let engine = reservation.engine();
        let ticks = batch
            .alternatives
            .iter()
            .find(|(e, _)| *e == engine)
            .ok_or_else(|| error("retained engine absent from chunk"))?
            .1;
        self.clock
            .tick_now()
            .tick()
            .checked_add(ticks)
            .ok_or_else(|| error("stream compute tick overflow"))?;
        let activity =
            ActivityLanes::begin_in_group(&self.activity_lanes.compute[engine.0], id, group);
        self.clock.wait_ticks(ticks)?.await;
        drop(activity);
        drop(reservation);
        let mut stats = self.stats.borrow_mut();
        if let Some(counters) = &mut stats.compute_actions {
            counters.record_batch(batch, engine)?;
        }
        stats.machine_ops = stats
            .machine_ops
            .clone()
            .checked_add(batch.machine_ops.clone())?;
        Ok(())
    }
    async fn run_stream_scalar_batch(
        &self,
        id: &str,
        batch: crate::processing_element::compute::EngineBatch,
        group: &Rc<gwr_track::entity::EntityGroup>,
    ) -> SimResult {
        use crate::processing_element::activity::ActivityLanes;
        let engines = batch
            .alternatives
            .iter()
            .map(|(e, _)| *e)
            .collect::<Vec<_>>();
        let reservation = self
            .compute_scheduler
            .acquire_any_with_accumulator(&engines, batch.matrix_accumulator_bytes)
            .await?;
        let engine = reservation.engine();
        let ticks = batch
            .alternatives
            .iter()
            .find(|(e, _)| *e == engine)
            .ok_or_else(|| error("truncate engine absent"))?
            .1;
        self.clock
            .tick_now()
            .tick()
            .checked_add(ticks)
            .ok_or_else(|| error("stream truncate tick overflow"))?;
        let activity =
            ActivityLanes::begin_in_group(&self.activity_lanes.compute[engine.0], id, group);
        self.clock.wait_ticks(ticks)?.await;
        drop(activity);
        drop(reservation);
        let mut stats = self.stats.borrow_mut();
        if let Some(counters) = &mut stats.compute_actions {
            counters.record_batch(&batch, engine)?;
        }
        stats.machine_ops = stats.machine_ops.clone().checked_add(batch.machine_ops)?;
        Ok(())
    }
}

impl std::fmt::Debug for LocalOperandHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalOperandHandle")
            .field("pair_offset", &self.pair_offset)
            .field("generation", &self.generation)
            .finish()
    }
}
struct PublishedScratch {
    scratch: ScratchLease,
    taken: std::collections::BTreeSet<usize>,
}
/// Immutable producer config; only the actual stream runner can publish its
/// scratch. Each original valid row/pair is consumed once. All issued handles
/// transfer into actor packets so caller cancellation cannot release backing
/// before the real publication ACK/canceled drain.
/// ```compile_fail
/// use gwr_models::processing_element::task::StreamedOutputSlot;
/// fn forge(slot:StreamedOutputSlot){slot.publish(()).unwrap();}
/// ```
pub struct StreamedOutputSlot {
    target: Option<Rc<crate::memory::atomic::AtomicTargetSender>>,
    issuer: Option<std::rc::Weak<crate::processing_element::ProcessingElement>>,
    config: StreamedMatrixTaskConfig,
    published: RefCell<Option<PublishedScratch>>,
    produced: std::cell::Cell<bool>,
}
impl std::fmt::Debug for StreamedOutputSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamedOutputSlot")
            .field("config", &self.config)
            .field("produced", &self.produced.get())
            .finish()
    }
}
impl StreamedOutputSlot {
    pub fn new_bound(
        config: StreamedMatrixTaskConfig,
        issuer: &Rc<crate::processing_element::ProcessingElement>,
        target: Rc<crate::memory::atomic::AtomicTargetSender>,
    ) -> Result<Rc<Self>, SimError> {
        issuer.preflight_streamed_matrix(&config)?;
        issuer.validate_streamed_target(&config, &target)?;
        Ok(Rc::new(Self {
            target: Some(target),
            issuer: Some(Rc::downgrade(issuer)),
            config,
            published: RefCell::new(None),
            produced: std::cell::Cell::new(false),
        }))
    }
    #[cfg(test)]
    fn new(config: StreamedMatrixTaskConfig) -> Result<Rc<Self>, SimError> {
        config.validate()?;
        Ok(Rc::new(Self {
            target: None,
            issuer: None,
            config,
            published: RefCell::new(None),
            produced: std::cell::Cell::new(false),
        }))
    }
    pub fn validate_for_pe(&self, pe: &crate::processing_element::ProcessingElement) -> SimResult {
        let issuer = self
            .issuer
            .as_ref()
            .and_then(std::rc::Weak::upgrade)
            .ok_or_else(|| error("stream producer requires actual PE binding"))?;
        if !std::ptr::eq(issuer.as_ref(), pe) {
            return Err(error("stream scratch cannot cross producer PE"));
        }
        pe.preflight_streamed_matrix(&self.config)?;
        pe.validate_streamed_target(
            &self.config,
            self.target
                .as_ref()
                .ok_or_else(|| error("stream lacks actual memory target"))?,
        )
    }
    pub(super) fn validate_runner(&self, entity: &Rc<gwr_track::entity::Entity>) -> SimResult {
        let issuer = self
            .issuer
            .as_ref()
            .and_then(std::rc::Weak::upgrade)
            .ok_or_else(|| error("stream producer requires actual PE binding"))?;
        if !Rc::ptr_eq(&issuer.entity, entity) {
            return Err(error("stream activated on foreign runner"));
        }
        Ok(())
    }
    pub fn config(&self) -> &StreamedMatrixTaskConfig {
        &self.config
    }
    pub(crate) fn validate_target(
        &self,
        target: &Rc<crate::memory::atomic::AtomicTargetSender>,
    ) -> SimResult {
        if !self
            .target
            .as_ref()
            .is_some_and(|bound| Rc::ptr_eq(bound, target))
        {
            return Err(error(
                "scratch target differs from actual shared source target",
            ));
        }
        Ok(())
    }
    pub(crate) fn validate_word(
        &self,
        effect: crate::memory::atomic::Effect,
        offset: usize,
    ) -> SimResult {
        let crate::memory::atomic::Effect::Atomic { participant, word } = effect else {
            return Err(error("scratch requires output atomic"));
        };
        let row = offset / 128;
        let col = (offset % 128) / 2;
        if !offset.is_multiple_of(4)
            || row >= self.config.m
            || col + 2 > 64
            || participant as usize != self.config.participant
            || (word.invocation, word.generation)
                != (self.config.invocation, self.config.generation)
        {
            return Err(error("scratch source role/valid pair mismatch"));
        }
        if self
            .target
            .as_ref()
            .is_some_and(|target| word.device != target.device())
        {
            return Err(error(
                "scratch output differs from actual source memory target",
            ));
        }
        let expected = self.config.c.tensor().addr()
            + ((row * self.config.n + self.config.n_offset + col) * 2) as u64;
        if word.address != expected {
            return Err(error("scratch pair differs from actual C source address"));
        }
        Ok(())
    }
    pub(crate) fn take(
        &self,
        effect: crate::memory::atomic::Effect,
        offset: usize,
    ) -> Result<Rc<LocalOperandHandle>, SimError> {
        self.validate_word(effect, offset)?;
        let mut state = self.published.borrow_mut();
        let p = state
            .as_mut()
            .ok_or_else(|| error("scratch producer not completed or all pairs consumed"))?;
        if !p.taken.insert(offset) {
            return Err(error("scratch pair already issued"));
        }
        let handle = Rc::new(p.scratch.pair(offset)?);
        if p.taken.len() == self.config.m * 32 {
            state.take();
        }
        Ok(handle)
    }
    pub(super) fn publish(&self, scratch: ScratchLease) -> SimResult {
        if self.produced.replace(true) {
            return Err(error("stream producer already completed"));
        }
        *self.published.borrow_mut() = Some(PublishedScratch {
            scratch,
            taken: std::collections::BTreeSet::default(),
        });
        Ok(())
    }
}
#[cfg(test)]
mod tests;
