// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Closed, caller-declared atomic/control protocol; no floating value backend.
use gwr_engine::types::{SimError, SimResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomicOperation {
    Bf16PairFadd,
    I32FetchAddOne,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomicScope {
    Agent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomicOrdering {
    Monotonic,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtomicWord {
    pub device: u64,
    pub address: u64,
    pub invocation: u64,
    pub generation: u64,
    pub operation: AtomicOperation,
    pub scope: AtomicScope,
    pub ordering: AtomicOrdering,
}
impl AtomicWord {
    pub fn validate(&self) -> SimResult {
        if !self.address.is_multiple_of(4)
            || self.address.checked_add(3).is_none()
            || self.generation == 0
        {
            return Err(SimError(
                "atomic target must be a complete aligned4B word in a positive generation".into(),
            ));
        }
        Ok(())
    }
}
/// Exact source signal flags, not invented LLVM acquire/release semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFlags {
    SplitKHgemmSc0Sc1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlGroup {
    pub invocation: u64,
    pub generation: u64,
    pub device: u64,
    pub signal: u64,
    pub semaphore: u64,
    pub participants: u32,
    pub publisher: u32,
    pub output_words: Vec<u64>,
    pub source_flags: SourceFlags,
}
impl ControlGroup {
    pub fn validate(&self) -> SimResult {
        if self.generation == 0
            || self.participants == 0
            || self.participants > 4096
            || self.publisher >= self.participants
            || self.signal == self.semaphore
            || self.output_words.is_empty()
            || self.output_words.len() > 4096
            || u128::from(self.participants) * self.output_words.len() as u128 > 65536
        {
            return Err(SimError("invalid control group roster".into()));
        }
        let mut words = std::collections::BTreeSet::new();
        for address in std::iter::once(&self.signal)
            .chain(std::iter::once(&self.semaphore))
            .chain(&self.output_words)
        {
            if address % 4 != 0 || address.checked_add(3).is_none() || !words.insert(*address) {
                return Err(SimError(
                    "control group has unaligned/duplicate/overlapping words".into(),
                ));
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlEffect {
    PublishSignalOne,
    PollSignal,
    FetchAddOne,
    ResetSemaphore,
    ResetSignal,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Effect {
    Atomic {
        word: AtomicWord,
        participant: u32,
    },
    Control {
        invocation: u64,
        generation: u64,
        participant: u32,
        effect: ControlEffect,
    },
    InitializeOutput {
        invocation: u64,
        generation: u64,
    },
    OrdinaryRead {
        bytes: u64,
    },
    OrdinaryWrite {
        bytes: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub identity: u64,
    pub device: u64,
    pub address: u64,
    pub effect: Effect,
}
impl Request {
    #[must_use]
    pub fn demand(&self) -> (u64, u64, u64) {
        match self.effect {
            Effect::Atomic { .. }
            | Effect::Control {
                effect: ControlEffect::FetchAddOne,
                ..
            } => (4, 4, 4),
            Effect::Control {
                effect: ControlEffect::PollSignal,
                ..
            } => (4, 0, 0),
            Effect::Control { .. } | Effect::InitializeOutput { .. } => (0, 4, 4),
            Effect::OrdinaryRead { bytes } => (bytes, 0, 0),
            Effect::OrdinaryWrite { bytes } => (0, bytes, bytes),
        }
    }
    pub fn end(&self) -> Result<u64, SimError> {
        let (read, write, _) = self.demand();
        self.address
            .checked_add(
                read.max(write)
                    .checked_sub(1)
                    .ok_or_else(|| SimError("zero payload".into()))?,
            )
            .ok_or_else(|| SimError("request address range overflows".into()))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtomicServiceConfig {
    pub capacity: usize,
    pub bytes_per_tick: u64,
    pub delay_ticks: u64,
    pub bf16_pair_per_tick: Option<u64>,
    pub i32_fetch_add_per_tick: Option<u64>,
    pub lane_interleave_bytes: u64,
    pub lane_bytes_per_tick: Vec<u64>,
}
impl AtomicServiceConfig {
    pub fn validate(&self) -> SimResult {
        if self.capacity == 0
            || self.capacity > 4096
            || self.bytes_per_tick == 0
            || self.bf16_pair_per_tick == Some(0)
            || self.i32_fetch_add_per_tick == Some(0)
            || self.lane_interleave_bytes < 4
            || !self.lane_interleave_bytes.is_power_of_two()
            || self.lane_bytes_per_tick.is_empty()
            || self.lane_bytes_per_tick.len() > 64
            || self.lane_bytes_per_tick.contains(&0)
        {
            return Err(SimError(
                "invalid caller atomic service capacity/rate/lanes".into(),
            ));
        }
        let sum = self
            .lane_bytes_per_tick
            .iter()
            .try_fold(0_u64, |a, b| a.checked_add(*b))
            .ok_or_else(|| SimError("lane rate sum overflows".into()))?;
        if sum != self.bytes_per_tick {
            return Err(SimError(
                "caller lane sum must equal shared byte rate".into(),
            ));
        }
        Ok(())
    }
}
/// Optional serialization carrier. It is not added to any vendor preset.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtomicPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<AtomicServiceConfig>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomicPath {
    CacheFreeDirect,
    Cache,
    Dma,
    RoutedOrdinary,
}
impl AtomicPath {
    pub fn validate(self) -> SimResult {
        if self != Self::CacheFreeDirect {
            return Err(SimError(
                "atomic protocol requires explicit cache-free direct ports".into(),
            ));
        }
        Ok(())
    }
}
