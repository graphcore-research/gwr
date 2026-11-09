// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Transactional finite service state. Canceled debt is never committed
//! traffic.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use gwr_engine::events::repeated::Repeated;
use gwr_engine::types::{SimError, SimResult};

use super::contract::{
    AtomicOperation, AtomicServiceConfig, ControlEffect, ControlGroup, Effect, Request,
};
fn error(message: &str) -> SimError {
    SimError(message.into())
}
fn add(a: u64, b: u64) -> Result<u64, SimError> {
    a.checked_add(b)
        .ok_or_else(|| error("atomic counter/deadline overflows"))
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtomicSnapshot {
    pub current: u64,
    pub peak: u64,
    pub queued: u64,
    pub serving: u64,
    pub committed_unacknowledged: u64,
    pub completed: u64,
    pub canceled_precommit_outstanding: u64,
    pub canceled_precommit_retired: u64,
    pub canceled_aftercommit: u64,
    pub canceled_aftercommit_outstanding: u64,
    pub accepted_requests: u64,
    pub accepted_operand_bytes: u64,
    pub reserved_service_bytes: u64,
    pub accepted_read_potential_bytes: u64,
    pub accepted_write_potential_bytes: u64,
    pub retired_service_debt_bytes: u64,
    pub committed_requests: u64,
    pub committed_read_bytes: u64,
    pub committed_write_bytes: u64,
    pub bf16_pair_requests: u64,
    pub bf16_element_updates: u64,
    pub i32_fetch_add_requests: u64,
    pub signal_poll_count: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub identity: u64,
    pub deadline: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    pub identity: u64,
    pub control_value: Option<u32>,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub response_bytes: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Queued,
    Serving,
    Committed,
    Publishing,
    CanceledReady,
    CanceledPublishing,
    CanceledDebt,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    request: Request,
    deadline: u64,
    phase: Phase,
    completion: Option<Completion>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Group {
    spec: ControlGroup,
    signal: u32,
    semaphore: u32,
    published: bool,
    initialized: BTreeSet<u64>,
    seen_one: BTreeSet<u32>,
    incremented: BTreeSet<u32>,
    increment_received: BTreeSet<u32>,
    last: Option<u32>,
    last_received: Option<u32>,
    reset_signal: bool,
    reset_semaphore: bool,
    output_commits: BTreeSet<(u32, u64)>,
    output_acks: BTreeSet<(u32, u64)>,
}
impl Group {
    fn finished(&self) -> bool {
        self.reset_signal
            && self.reset_semaphore
            && self.output_acks.len() as u128
                == u128::from(self.spec.participants) * self.spec.output_words.len() as u128
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DebugState {
    snapshot: AtomicSnapshot,
    entries: Vec<Option<Entry>>,
    next_identity: u64,
    byte_end: u128,
    lane_end: Vec<u128>,
    pair_end: u128,
    integer_end: u128,
    groups: BTreeMap<u64, Group>,
}
/// Shared budget for ordinary, atomic and typed control effects. Callers choose
/// every service rate; no physical atomic throughput or numeric BF16 is
/// implied.
pub struct AtomicService {
    device: u64,
    base: u64,
    last: u64,
    config: AtomicServiceConfig,
    state: RefCell<DebugState>,
    pub(super) changed: Repeated<()>,
}
impl AtomicService {
    pub fn new(
        device: u64,
        base: u64,
        bytes: u64,
        config: AtomicServiceConfig,
    ) -> Result<Self, SimError> {
        config.validate()?;
        let last = base
            .checked_add(
                bytes
                    .checked_sub(1)
                    .ok_or_else(|| error("empty atomic memory"))?,
            )
            .ok_or_else(|| error("atomic memory range overflows"))?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(config.capacity)
            .map_err(|e| SimError(e.to_string()))?;
        entries.resize(config.capacity, None);
        let lane_end = vec![0; config.lane_bytes_per_tick.len()];
        Ok(Self {
            device,
            base,
            last,
            config,
            state: RefCell::new(DebugState {
                entries,
                lane_end,
                next_identity: 1,
                ..Default::default()
            }),
            changed: Repeated::default(),
        })
    }
    pub fn snapshot(&self) -> AtomicSnapshot {
        self.state.borrow().snapshot
    }
    pub(super) fn capacity(&self) -> usize {
        self.config.capacity
    }
    pub fn debug_state(&self) -> DebugState {
        self.state.borrow().clone()
    }
    pub fn register_control(&self, spec: ControlGroup) -> SimResult {
        spec.validate()?;
        if spec.device != self.device {
            return Err(error("control group memory identity mismatch"));
        }
        let mut next = self.state.borrow().clone();
        if let Some(old) = next.groups.get(&spec.invocation) {
            if !old.finished() || spec.generation != add(old.spec.generation, 1)? {
                return Err(error(
                    "control generation reuse before final output ACK or stale generation",
                ));
            }
            if next.entries.iter().flatten().any(|e| {
                request_generation(&e.request) == Some((old.spec.invocation, old.spec.generation))
            }) {
                return Err(error(
                    "control generation still owns outstanding request/debt/reply",
                ));
            }
        } else if next.groups.len() >= self.config.capacity {
            return Err(error("control group budget exhausted"));
        }
        let words: BTreeSet<_> = std::iter::once(spec.signal)
            .chain(std::iter::once(spec.semaphore))
            .chain(spec.output_words.iter().copied())
            .collect();
        if words
            .iter()
            .any(|a| *a < self.base || a.checked_add(3).is_none_or(|end| end > self.last))
        {
            return Err(error("control word out of bounds"));
        }
        for group in next
            .groups
            .values()
            .filter(|g| g.spec.invocation != spec.invocation && !g.finished())
        {
            if std::iter::once(group.spec.signal)
                .chain(std::iter::once(group.spec.semaphore))
                .chain(group.spec.output_words.iter().copied())
                .any(|a| words.contains(&a))
            {
                return Err(error("control words already owned by another invocation"));
            }
        }
        if next.entries.iter().flatten().any(|e| {
            words.iter().any(|a| {
                *a <= e.request.end().unwrap_or(u64::MAX)
                    && add(*a, 3).unwrap_or(u64::MAX) >= e.request.address
            })
        }) {
            return Err(error("control registration overlaps live request"));
        }
        next.groups.insert(
            spec.invocation,
            Group {
                spec,
                signal: 0,
                semaphore: 0,
                published: false,
                initialized: BTreeSet::new(),
                seen_one: BTreeSet::new(),
                incremented: BTreeSet::new(),
                increment_received: BTreeSet::new(),
                last: None,
                last_received: None,
                reset_signal: false,
                reset_semaphore: false,
                output_commits: BTreeSet::new(),
                output_acks: BTreeSet::new(),
            },
        );
        *self.state.borrow_mut() = next;
        Ok(())
    }
    fn group(state: &DebugState, invocation: u64, generation: u64) -> Result<&Group, SimError> {
        state
            .groups
            .get(&invocation)
            .filter(|g| g.spec.generation == generation)
            .ok_or_else(|| error("missing/foreign control generation"))
    }
    #[expect(
        clippy::too_many_lines,
        reason = "Keep the typed request validation and admission contract together"
    )]
    fn validate_request(&self, state: &DebugState, request: &Request) -> SimResult {
        let end = request.end()?;
        if request.device != self.device || request.address < self.base || end > self.last {
            return Err(error("request memory identity/range mismatch"));
        }
        if !matches!(
            request.effect,
            Effect::OrdinaryRead { .. } | Effect::OrdinaryWrite { .. }
        ) && (!request.address.is_multiple_of(4) || end != add(request.address, 3)?)
        {
            return Err(error("typed effect requires exact aligned4B word"));
        }
        for group in state.groups.values().filter(|g| !g.finished()) {
            let overlaps = std::iter::once(group.spec.signal)
                .chain(std::iter::once(group.spec.semaphore))
                .chain(group.spec.output_words.iter().copied())
                .any(|a| a <= end && a + 3 >= request.address);
            if overlaps
                && matches!(
                    request.effect,
                    Effect::OrdinaryRead { .. } | Effect::OrdinaryWrite { .. }
                )
            {
                return Err(error("ordinary access aliases owned control/atomic word"));
            }
        }
        match &request.effect {
            Effect::Atomic { word, participant } => {
                word.validate()?;
                for entry in state.entries.iter().flatten().filter(|e| {
                    matches!(
                        e.phase,
                        Phase::Queued | Phase::Serving | Phase::CanceledDebt
                    )
                }) {
                    if let Effect::Atomic { word: other, .. } = entry.request.effect
                        && other.address == word.address
                        && other != *word
                    {
                        return Err(error(
                            "live atomic word has incompatible generation/signature",
                        ));
                    }
                }
                if word.device != request.device
                    || word.address != request.address
                    || word.operation != AtomicOperation::Bf16PairFadd
                {
                    return Err(error("atomic word signature or operation mismatch"));
                }
                if self.config.bf16_pair_per_tick.is_none() {
                    return Err(error("missing BF16 pair atomic capability"));
                }
                if let Some(g) = state.groups.get(&word.invocation) {
                    if g.spec.generation != word.generation
                        || !g.spec.output_words.contains(&word.address)
                        || *participant >= g.spec.participants
                        || !g.increment_received.contains(participant)
                        || g.output_commits.contains(&(*participant, word.address))
                    {
                        return Err(error("foreign/early/duplicate output atomic"));
                    }
                    if g.last == Some(*participant) && (!g.reset_semaphore || !g.reset_signal) {
                        return Err(error("last-arrival output precedes control reset"));
                    }
                }
                for g in state.groups.values().filter(|g| !g.finished()) {
                    if g.spec.output_words.contains(&word.address) {
                        if g.spec.invocation != word.invocation
                            || g.spec.generation != word.generation
                            || *participant >= g.spec.participants
                            || !g.increment_received.contains(participant)
                            || g.output_commits.contains(&(*participant, word.address))
                        {
                            return Err(error("foreign/early/duplicate output atomic"));
                        }
                    } else if word.address == g.spec.signal || word.address == g.spec.semaphore {
                        return Err(error("floating atomic aliases control word"));
                    }
                }
            }
            Effect::InitializeOutput {
                invocation,
                generation,
            } => {
                let g = Self::group(state, *invocation, *generation)?;
                if !g.spec.output_words.contains(&request.address)
                    || g.published
                    || g.initialized.contains(&request.address)
                {
                    return Err(error("invalid/duplicate output initialization"));
                }
            }
            Effect::Control {
                invocation,
                generation,
                participant,
                effect,
            } => {
                let g = Self::group(state, *invocation, *generation)?;
                if *participant >= g.spec.participants {
                    return Err(error("control participant out of bounds"));
                }
                let target = match effect {
                    ControlEffect::FetchAddOne | ControlEffect::ResetSemaphore => g.spec.semaphore,
                    _ => g.spec.signal,
                };
                if target != request.address {
                    return Err(error("control effect target mismatch"));
                }
                match effect {
                    ControlEffect::PublishSignalOne
                        if *participant != g.spec.publisher
                            || g.published
                            || g.initialized.len() != g.spec.output_words.len() =>
                    {
                        return Err(error("duplicate/early/foreign publisher"));
                    }
                    ControlEffect::FetchAddOne
                        if !g.seen_one.contains(participant)
                            || g.incremented.contains(participant)
                            || self.config.i32_fetch_add_per_tick.is_none() =>
                    {
                        return Err(error(
                            "missing fetch-add capability or duplicate/early participant",
                        ));
                    }
                    ControlEffect::ResetSemaphore
                        if g.last_received != Some(*participant) || g.reset_semaphore =>
                    {
                        return Err(error("premature/duplicate semaphore reset"));
                    }
                    ControlEffect::ResetSignal
                        if g.last_received != Some(*participant)
                            || !g.reset_semaphore
                            || g.reset_signal =>
                    {
                        return Err(error("premature/duplicate signal reset"));
                    }
                    ControlEffect::PollSignal if g.incremented.contains(participant) => {
                        return Err(error("signal poll after participant increment"));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        // Do not admit a duplicate role while an earlier request is reserved.
        for entry in state.entries.iter().flatten() {
            if matches!(
                entry.phase,
                Phase::CanceledDebt
                    | Phase::Committed
                    | Phase::Publishing
                    | Phase::CanceledReady
                    | Phase::CanceledPublishing
            ) {
                continue;
            }
            if entry.request.address == request.address
                && entry.request.effect == request.effect
                && !matches!(
                    request.effect,
                    Effect::OrdinaryRead { .. }
                        | Effect::OrdinaryWrite { .. }
                        | Effect::Control {
                            effect: ControlEffect::PollSignal,
                            ..
                        }
                )
            {
                if let Effect::Atomic { word, .. } = request.effect
                    && !state.groups.contains_key(&word.invocation)
                {
                    continue;
                }
                return Err(error("duplicate reserved effect"));
            }
        }
        Ok(())
    }
    #[expect(
        clippy::too_many_lines,
        reason = "Keep the typed request validation and admission contract together"
    )]
    pub fn try_accept(&self, arrival: u64, request: Request) -> Result<Ticket, SimError> {
        let mut next = self.state.borrow().clone();
        self.validate_request(&next, &request)?;
        if request.identity != next.next_identity {
            return Err(error("request identity must be next unique ordinal"));
        }
        let slot = next
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or_else(|| error("atomic owner capacity exhausted"))?;
        let (read, write, operand) = request.demand();
        let demand = add(read, write)?;
        let byte_deadline = reserve_debt(
            &mut next.byte_end,
            self.config.bytes_per_tick,
            arrival,
            demand,
        )?;
        let mut deadline = byte_deadline;
        let multiplicity = if read != 0 && write != 0 { 2 } else { 1 };
        let lane_bytes = super::super::payload_by_lane(
            self.base,
            request.address,
            usize::try_from(read.max(write)).map_err(|_| error("payload exceeds usize"))?,
            self.config.lane_interleave_bytes,
            next.lane_end.len(),
        )?;
        for ((end, rate), bytes) in next
            .lane_end
            .iter_mut()
            .zip(&self.config.lane_bytes_per_tick)
            .zip(lane_bytes)
        {
            if bytes != 0 {
                deadline = deadline.max(reserve_debt(
                    end,
                    *rate,
                    arrival,
                    u64::try_from(bytes)
                        .map_err(|_| error("lane bytes exceed u64"))?
                        .checked_mul(multiplicity)
                        .ok_or_else(|| error("lane RMW bytes overflow"))?,
                )?);
            }
        }
        match request.effect {
            Effect::Atomic { .. } => {
                deadline = deadline.max(reserve_debt(
                    &mut next.pair_end,
                    self.config.bf16_pair_per_tick.unwrap(),
                    arrival,
                    1,
                )?);
            }
            Effect::Control {
                effect: ControlEffect::FetchAddOne,
                ..
            } => {
                deadline = deadline.max(reserve_debt(
                    &mut next.integer_end,
                    self.config.i32_fetch_add_per_tick.unwrap(),
                    arrival,
                    1,
                )?);
            }
            _ => {}
        }
        deadline = add(deadline, self.config.delay_ticks)?;
        // Service resources are charged once. Dependency eligibility can delay
        // a fast ordinary/control request behind a slower overlapping RMW.
        let end = request.end()?;
        for prior in next.entries.iter().flatten().filter(|e| {
            matches!(
                e.phase,
                Phase::Queued | Phase::Serving | Phase::CanceledDebt
            )
        }) {
            if prior.request.address <= end && request.address <= prior.request.end()? {
                deadline = deadline.max(prior.deadline);
            }
        }
        next.next_identity = add(next.next_identity, 1)?;
        let s = &mut next.snapshot;
        s.accepted_requests = add(s.accepted_requests, 1)?;
        s.accepted_operand_bytes = add(s.accepted_operand_bytes, operand)?;
        s.reserved_service_bytes = add(s.reserved_service_bytes, demand)?;
        s.accepted_read_potential_bytes = add(s.accepted_read_potential_bytes, read)?;
        s.accepted_write_potential_bytes = add(s.accepted_write_potential_bytes, write)?;
        s.current = add(s.current, 1)?;
        s.queued = add(s.queued, 1)?;
        s.peak = s.peak.max(s.current);
        let ticket = Ticket {
            identity: request.identity,
            deadline,
        };
        next.entries[slot] = Some(Entry {
            request,
            deadline,
            phase: Phase::Queued,
            completion: None,
        });
        *self.state.borrow_mut() = next;
        self.changed.notify();
        Ok(ticket)
    }
    pub fn start(&self, identity: u64) -> SimResult {
        let mut next = self.state.borrow().clone();
        let i = entry_index(&next, identity)?;
        if next.entries[i].as_ref().unwrap().phase != Phase::Queued {
            return Err(error("owner not queued"));
        }
        next.entries[i].as_mut().unwrap().phase = Phase::Serving;
        next.snapshot.queued -= 1;
        next.snapshot.serving += 1;
        *self.state.borrow_mut() = next;
        Ok(())
    }
    pub fn commit(&self, now: u64, identity: u64) -> Result<Completion, SimError> {
        let mut next = self.state.borrow().clone();
        let i = entry_index(&next, identity)?;
        let entry = next.entries[i].as_ref().unwrap().clone();
        if now < entry.deadline || !matches!(entry.phase, Phase::Queued | Phase::Serving) {
            return Err(error("commit before deadline or nonlive owner"));
        }
        if next.entries.iter().flatten().any(|e| {
            e.request.identity < identity
                && e.request.address <= entry.request.end().unwrap_or(u64::MAX)
                && entry.request.address <= e.request.end().unwrap_or(u64::MAX)
                && matches!(
                    e.phase,
                    Phase::Queued | Phase::Serving | Phase::CanceledDebt
                )
        }) {
            return Err(error("earlier same-word owner not committed/debt-retired"));
        }
        self.validate_request_for_commit(&next, i)?;
        let value = apply_effect(&mut next, &entry.request)?;
        let (read, write, _) = entry.request.demand();
        let demand = add(read, write)?;
        let s = &mut next.snapshot;
        s.committed_requests = add(s.committed_requests, 1)?;
        s.committed_read_bytes = add(s.committed_read_bytes, read)?;
        s.committed_write_bytes = add(s.committed_write_bytes, write)?;
        s.retired_service_debt_bytes = add(s.retired_service_debt_bytes, demand)?;
        if entry.phase == Phase::Queued {
            s.queued -= 1;
        } else {
            s.serving -= 1;
        }
        s.committed_unacknowledged += 1;
        match entry.request.effect {
            Effect::Atomic { .. } => {
                s.bf16_pair_requests = add(s.bf16_pair_requests, 1)?;
                s.bf16_element_updates = add(s.bf16_element_updates, 2)?;
            }
            Effect::Control {
                effect: ControlEffect::FetchAddOne,
                ..
            } => s.i32_fetch_add_requests = add(s.i32_fetch_add_requests, 1)?,
            Effect::Control {
                effect: ControlEffect::PollSignal,
                ..
            } => s.signal_poll_count = add(s.signal_poll_count, 1)?,
            _ => {}
        }
        let response_bytes = match entry.request.effect {
            Effect::Atomic { .. } => 0,
            Effect::Control {
                effect: ControlEffect::FetchAddOne | ControlEffect::PollSignal,
                ..
            } => 4,
            Effect::OrdinaryRead { bytes } => bytes,
            _ => 0,
        };
        let completion = Completion {
            identity,
            control_value: value,
            read_bytes: read,
            write_bytes: write,
            response_bytes,
        };
        let e = next.entries[i].as_mut().unwrap();
        e.phase = Phase::Committed;
        e.completion = Some(completion);
        *self.state.borrow_mut() = next;
        self.changed.notify();
        Ok(completion)
    }
    fn validate_request_for_commit(&self, state: &DebugState, index: usize) -> SimResult {
        let mut without_self = state.clone();
        let request = without_self.entries[index].take().unwrap().request;
        self.validate_request(&without_self, &request)
    }
    pub fn cancel(&self, identity: u64) -> SimResult {
        let mut next = self.state.borrow().clone();
        let i = entry_index(&next, identity)?;
        let entry = next.entries[i].as_ref().unwrap().clone();
        match entry.phase {
            Phase::CanceledDebt => return Err(error("owner already canceled")),
            Phase::CanceledPublishing => return Err(error("published owner already canceled")),
            Phase::CanceledReady => return Err(error("ready owner already canceled")),
            Phase::Publishing => {
                // An existing PortPut cannot be withdrawn/reused safely after
                // grant. Retain credit and the actual put through client ACK.
                next.entries[i].as_mut().unwrap().phase = Phase::CanceledPublishing;
                next.snapshot.committed_unacknowledged -= 1;
                next.snapshot.canceled_aftercommit_outstanding += 1;
            }
            Phase::Committed => {
                next.entries[i].as_mut().unwrap().phase = Phase::CanceledReady;
                next.snapshot.committed_unacknowledged -= 1;
                next.snapshot.canceled_aftercommit_outstanding += 1;
            }
            Phase::Queued | Phase::Serving => {
                if entry.phase == Phase::Queued {
                    next.snapshot.queued -= 1;
                } else {
                    next.snapshot.serving -= 1;
                }
                next.entries[i].as_mut().unwrap().phase = Phase::CanceledDebt;
                next.snapshot.canceled_precommit_outstanding += 1;
            }
        }
        *self.state.borrow_mut() = next;
        self.changed.notify();
        Ok(())
    }
    pub fn retire_canceled(&self, now: u64) -> SimResult {
        let mut next = self.state.borrow().clone();
        let mut retired = false;
        for i in 0..next.entries.len() {
            if let Some(e) = &next.entries[i] {
                if e.phase != Phase::CanceledDebt || now < e.deadline {
                    continue;
                }
                let (read, write, _) = e.request.demand();
                next.snapshot.retired_service_debt_bytes =
                    add(next.snapshot.retired_service_debt_bytes, add(read, write)?)?;
                next.snapshot.canceled_precommit_outstanding -= 1;
                next.snapshot.current -= 1;
                next.snapshot.canceled_precommit_retired =
                    add(next.snapshot.canceled_precommit_retired, 1)?;
                next.entries[i] = None;
                retired = true;
            }
        }
        if retired {
            *self.state.borrow_mut() = next;
            self.changed.notify();
        }
        Ok(())
    }
    pub fn acknowledge(&self, identity: u64) -> SimResult {
        let mut next = self.state.borrow().clone();
        let i = entry_index(&next, identity)?;
        let e = next.entries[i].as_ref().unwrap().clone();
        if !matches!(
            e.phase,
            Phase::Committed | Phase::Publishing | Phase::CanceledPublishing
        ) {
            return Err(error("ACK requires committed response"));
        }
        if e.phase != Phase::CanceledPublishing {
            record_control_ack(&mut next, &e.request, e.completion.unwrap())?;
        }
        record_output_ack(&mut next, &e.request);
        next.entries[i] = None;
        if e.phase == Phase::CanceledPublishing {
            next.snapshot.canceled_aftercommit_outstanding -= 1;
            next.snapshot.canceled_aftercommit = add(next.snapshot.canceled_aftercommit, 1)?;
        } else {
            next.snapshot.committed_unacknowledged -= 1;
            next.snapshot.completed = add(next.snapshot.completed, 1)?;
        }
        next.snapshot.current -= 1;
        *self.state.borrow_mut() = next;
        self.changed.notify();
        Ok(())
    }
    /// The sole publisher calls this only after removing the actual ready
    /// payload. Cancel alone cannot recycle a slot while that payload remains.
    pub(super) fn discard_canceled_response(&self, identity: u64) -> SimResult {
        let mut next = self.state.borrow().clone();
        let i = entry_index(&next, identity)?;
        if next.entries[i].as_ref().unwrap().phase != Phase::CanceledReady {
            return Err(error("discard requires canceled ready response"));
        }
        next.entries[i] = None;
        next.snapshot.current -= 1;
        next.snapshot.canceled_aftercommit_outstanding -= 1;
        next.snapshot.canceled_aftercommit = add(next.snapshot.canceled_aftercommit, 1)?;
        *self.state.borrow_mut() = next;
        self.changed.notify();
        Ok(())
    }
    pub(super) fn canceled_publication(&self, identity: u64) -> bool {
        self.state
            .borrow()
            .entries
            .iter()
            .flatten()
            .any(|e| e.request.identity == identity && e.phase == Phase::CanceledPublishing)
    }
    pub(super) fn canceled_ready(&self, identity: u64) -> bool {
        self.state
            .borrow()
            .entries
            .iter()
            .flatten()
            .any(|e| e.request.identity == identity && e.phase == Phase::CanceledReady)
    }
    pub fn control_finished(&self, invocation: u64, generation: u64) -> Result<bool, SimError> {
        Ok(Self::group(&self.state.borrow(), invocation, generation)?.finished())
    }
    pub(super) fn pending(&self) -> Vec<Ticket> {
        self.state
            .borrow()
            .entries
            .iter()
            .flatten()
            .filter(|e| {
                matches!(
                    e.phase,
                    Phase::Queued | Phase::Serving | Phase::CanceledDebt
                )
            })
            .map(|e| Ticket {
                identity: e.request.identity,
                deadline: e.deadline,
            })
            .collect()
    }
    pub(super) fn start_if_queued(&self, identity: u64) -> SimResult {
        let state = self.state.borrow();
        let i = entry_index(&state, identity)?;
        let queued = state.entries[i].as_ref().unwrap().phase == Phase::Queued;
        drop(state);
        if queued {
            self.start(identity)?;
        }
        Ok(())
    }
    pub fn begin_publication(&self, identity: u64) -> SimResult {
        let mut state = self.state.borrow_mut();
        let i = entry_index(&state, identity)?;
        if state.entries[i].as_ref().unwrap().phase != Phase::Committed {
            return Err(error("publication requires ready committed owner"));
        }
        state.entries[i].as_mut().unwrap().phase = Phase::Publishing;
        Ok(())
    }
    pub(super) fn canceled_or_retired(&self, identity: u64) -> bool {
        self.state
            .borrow()
            .entries
            .iter()
            .flatten()
            .find(|e| e.request.identity == identity)
            .is_none_or(|e| e.phase == Phase::CanceledDebt)
    }
}
fn entry_index(state: &DebugState, identity: u64) -> Result<usize, SimError> {
    state
        .entries
        .iter()
        .position(|e| e.as_ref().is_some_and(|e| e.request.identity == identity))
        .ok_or_else(|| error("unknown/retired owner"))
}
fn reserve_debt(end: &mut u128, rate: u64, arrival: u64, demand: u64) -> Result<u64, SimError> {
    if rate == 0 || demand == 0 {
        return Err(error(
            "service debt must be strictly positive at finite rate",
        ));
    }
    let arrival = u128::from(arrival)
        .checked_mul(u128::from(rate))
        .ok_or_else(|| error("arrival debt overflows"))?;
    let next = (*end)
        .max(arrival)
        .checked_add(u128::from(demand))
        .ok_or_else(|| error("service debt overflows"))?;
    let rounded = next / u128::from(rate) + u128::from(next % u128::from(rate) != 0);
    let deadline = u64::try_from(rounded).map_err(|_| error("deadline exceeds u64"))?;
    *end = next;
    Ok(deadline)
}
fn apply_effect(state: &mut DebugState, request: &Request) -> Result<Option<u32>, SimError> {
    match request.effect {
        Effect::InitializeOutput { invocation, .. } => {
            state
                .groups
                .get_mut(&invocation)
                .unwrap()
                .initialized
                .insert(request.address);
            Ok(None)
        }
        Effect::Atomic { word, participant } => {
            if let Some(g) = state.groups.get_mut(&word.invocation)
                && g.spec.generation == word.generation
                && g.spec.output_words.contains(&word.address)
            {
                g.output_commits.insert((participant, word.address));
            }
            Ok(None)
        }
        Effect::Control {
            invocation,
            participant,
            effect,
            ..
        } => {
            let g = state.groups.get_mut(&invocation).unwrap();
            match effect {
                ControlEffect::PublishSignalOne => {
                    g.signal = 1;
                    g.published = true;
                    Ok(None)
                }
                ControlEffect::PollSignal => Ok(Some(g.signal)),
                ControlEffect::FetchAddOne => {
                    let old = g.semaphore;
                    g.semaphore = old
                        .checked_add(1)
                        .ok_or_else(|| error("semaphore count overflows"))?;
                    g.incremented.insert(participant);
                    if old == g.spec.participants - 1 {
                        g.last = Some(participant);
                    }
                    Ok(Some(old))
                }
                ControlEffect::ResetSemaphore => {
                    g.semaphore = 0;
                    g.reset_semaphore = true;
                    Ok(None)
                }
                ControlEffect::ResetSignal => {
                    g.signal = 0;
                    g.reset_signal = true;
                    Ok(None)
                }
            }
        }
        _ => Ok(None),
    }
}
fn record_output_ack(state: &mut DebugState, request: &Request) {
    if let Effect::Atomic { word, participant } = request.effect
        && let Some(g) = state.groups.get_mut(&word.invocation)
        && g.spec.generation == word.generation
        && g.spec.output_words.contains(&word.address)
    {
        g.output_acks.insert((participant, word.address));
    }
}
fn request_generation(request: &Request) -> Option<(u64, u64)> {
    match request.effect {
        Effect::Atomic { word, .. } => Some((word.invocation, word.generation)),
        Effect::Control {
            invocation,
            generation,
            ..
        }
        | Effect::InitializeOutput {
            invocation,
            generation,
        } => Some((invocation, generation)),
        _ => None,
    }
}
fn record_control_ack(
    state: &mut DebugState,
    request: &Request,
    completion: Completion,
) -> SimResult {
    if let Effect::Control {
        invocation,
        generation,
        participant,
        effect,
    } = request.effect
    {
        let g = state
            .groups
            .get_mut(&invocation)
            .filter(|g| g.spec.generation == generation)
            .ok_or_else(|| error("stale/foreign control reply ACK"))?;
        if effect == ControlEffect::PollSignal && completion.control_value == Some(1) {
            g.seen_one.insert(participant);
        }
        if effect == ControlEffect::FetchAddOne {
            g.increment_received.insert(participant);
            if completion.control_value == Some(g.spec.participants - 1) {
                g.last_received = Some(participant);
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::atomic::{AtomicOrdering, AtomicScope, AtomicWord};
    fn service() -> AtomicService {
        AtomicService::new(
            7,
            0x1000,
            256,
            AtomicServiceConfig {
                capacity: 2,
                bytes_per_tick: 8,
                delay_ticks: 0,
                bf16_pair_per_tick: Some(1),
                i32_fetch_add_per_tick: Some(1),
                lane_interleave_bytes: 4,
                lane_bytes_per_tick: vec![8],
            },
        )
        .unwrap()
    }
    fn pair(id: u64) -> Request {
        Request {
            identity: id,
            device: 7,
            address: 0x1000,
            effect: Effect::Atomic {
                word: AtomicWord {
                    device: 7,
                    address: 0x1000,
                    generation: 1,
                    invocation: 9,
                    operation: AtomicOperation::Bf16PairFadd,
                    scope: AtomicScope::Agent,
                    ordering: AtomicOrdering::Monotonic,
                },
                participant: 0,
            },
        }
    }
    #[test]
    fn synthetic_exhaustion_seeds_reject_without_partial_admission_or_commit() {
        // Happy baseline first. Seeds below represent metadata exhaustion only,
        // never an enormous physical transaction or a completed native run.
        let good = service();
        let ticket = good.try_accept(0, pair(1)).unwrap();
        good.commit(1, ticket.identity).unwrap();
        good.acknowledge(1).unwrap();
        let selectors: [fn(&mut AtomicSnapshot) -> &mut u64; 5] = [
            |s| &mut s.accepted_requests,
            |s| &mut s.accepted_operand_bytes,
            |s| &mut s.reserved_service_bytes,
            |s| &mut s.accepted_read_potential_bytes,
            |s| &mut s.accepted_write_potential_bytes,
        ];
        for select in selectors {
            let s = service();
            *select(&mut s.state.borrow_mut().snapshot) = u64::MAX;
            let before = s.debug_state();
            assert!(s.try_accept(0, pair(1)).is_err());
            assert_eq!(s.debug_state(), before);
        }
        for cursor in 0..3 {
            let s = service();
            {
                let mut state = s.state.borrow_mut();
                match cursor {
                    0 => state.byte_end = u128::MAX,
                    1 => state.lane_end[0] = u128::MAX,
                    _ => state.pair_end = u128::MAX,
                }
            }
            let before = s.debug_state();
            assert!(s.try_accept(0, pair(1)).is_err());
            assert_eq!(s.debug_state(), before);
        }
        let selectors: [fn(&mut AtomicSnapshot) -> &mut u64; 6] = [
            |s| &mut s.committed_requests,
            |s| &mut s.committed_read_bytes,
            |s| &mut s.committed_write_bytes,
            |s| &mut s.retired_service_debt_bytes,
            |s| &mut s.bf16_pair_requests,
            |s| &mut s.bf16_element_updates,
        ];
        for select in selectors {
            let s = service();
            s.try_accept(0, pair(1)).unwrap();
            *select(&mut s.state.borrow_mut().snapshot) = u64::MAX;
            let before = s.debug_state();
            assert!(s.commit(1, 1).is_err());
            assert_eq!(s.debug_state(), before);
        }
        let s = service();
        s.state.borrow_mut().next_identity = u64::MAX;
        let before = s.debug_state();
        assert!(s.try_accept(0, pair(u64::MAX)).is_err());
        assert_eq!(s.debug_state(), before);
    }
}
