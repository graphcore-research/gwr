// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Opt-in ordinary/typed ports with ONE service and bounded ownership ledger.
//! Protocol effects only: no numeric BF16 values or vendor rates.
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::rc::Rc;

use futures::channel::oneshot;
use futures::future::{Either, select};
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Event, SimObject, TotalBytes};
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_track::Id;
use gwr_track::entity::Entity;
use gwr_track::id::Unique;

use super::{
    AtomicOperation, AtomicPath, AtomicReply, AtomicService, AtomicServiceConfig, AtomicSnapshot,
    Completion, ControlEffect, ControlGroup, Effect, Request,
};
use crate::memory::traits::AccessMemory;
fn error(s: &str) -> SimError {
    SimError(s.into())
}

struct TargetDescription {
    device: u64,
    base: u64,
    last: u64,
    config: AtomicServiceConfig,
    groups: RefCell<BTreeMap<u64, ControlGroup>>,
    ordinary_input: RefCell<Option<Rc<Entity>>>,
    ordinary_output: RefCell<Option<Rc<Entity>>>,
}
impl TargetDescription {
    // Static signature validation only. No signal values, pending owners,
    // future participant receipt, request IDs, clocks or service mutation.
    fn validate(&self, device: u64, address: u64, effect: Effect) -> SimResult {
        if device != self.device
            || address < self.base
            || !address.is_multiple_of(4)
            || address.checked_add(3).is_none_or(|a| a > self.last)
        {
            return Err(error("typed target device/aligned word/range mismatch"));
        }
        let groups = self.groups.borrow();
        match effect {
            Effect::Atomic { word, participant } => {
                word.validate()?;
                if word.device != device
                    || word.address != address
                    || word.operation != AtomicOperation::Bf16PairFadd
                    || self.config.bf16_pair_per_tick.is_none()
                {
                    return Err(error("typed pair signature/capability mismatch"));
                }
                if let Some(g) = groups.get(&word.invocation)
                    && (g.generation != word.generation
                        || participant >= g.participants
                        || !g.output_words.contains(&address))
                {
                    return Err(error("typed output group/participant/generation mismatch"));
                }
            }
            Effect::Control {
                invocation,
                generation,
                participant,
                effect,
            } => {
                let g = groups
                    .get(&invocation)
                    .filter(|g| g.generation == generation)
                    .ok_or_else(|| error("typed control lacks registered exact generation"))?;
                if participant >= g.participants {
                    return Err(error(
                        "static dispatch requires an actual participant; dynamic last is Task2",
                    ));
                }
                let expected = match effect {
                    ControlEffect::PollSignal
                    | ControlEffect::PublishSignalOne
                    | ControlEffect::ResetSignal => g.signal,
                    ControlEffect::FetchAddOne | ControlEffect::ResetSemaphore => g.semaphore,
                };
                if address != expected
                    || (effect == ControlEffect::PublishSignalOne && participant != g.publisher)
                    || (effect == ControlEffect::FetchAddOne
                        && self.config.i32_fetch_add_per_tick.is_none())
                {
                    return Err(error("typed control role/target/capability mismatch"));
                }
            }
            Effect::InitializeOutput {
                invocation,
                generation,
            } => {
                let g = groups
                    .get(&invocation)
                    .filter(|g| g.generation == generation)
                    .ok_or_else(|| error("typed initialization lacks exact generation"))?;
                if !g.output_words.contains(&address) {
                    return Err(error("typed initialization target mismatch"));
                }
            }
            Effect::OrdinaryRead { .. } | Effect::OrdinaryWrite { .. } => {
                return Err(error(
                    "ordinary requests must use their actual ordinary memory port",
                ));
            }
        }
        Ok(())
    }
}

type Finished = Rc<RefCell<Option<oneshot::Sender<SimResult>>>>;
#[derive(Clone)]
struct TypedInput {
    request: Request,
    finished: Finished,
    operand: Option<Rc<crate::processing_element::LocalOperandHandle>>,
}
impl fmt::Debug for TypedInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.request.fmt(f)
    }
}
impl fmt::Display for TypedInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "typed {}", self.request.identity)
    }
}
impl Unique for TypedInput {
    fn id(&self) -> Id {
        Id(self.request.identity)
    }
}
impl TotalBytes for TypedInput {
    fn total_bytes(&self) -> usize {
        self.request.demand().2 as usize
    }
}
impl SimObject for TypedInput {}

#[derive(Clone, Debug)]
struct SharedReply {
    reply: AtomicReply,
    owner: u64,
    _operand: Option<Rc<crate::processing_element::LocalOperandHandle>>,
}
impl fmt::Display for SharedReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reply.fmt(f)
    }
}
impl Unique for SharedReply {
    fn id(&self) -> Id {
        self.reply.id()
    }
}
impl TotalBytes for SharedReply {
    fn total_bytes(&self) -> usize {
        self.reply.total_bytes()
    }
}
impl SimObject for SharedReply {}
// Unlike a guard constructed before a pending request, this credit exists
// only AFTER a successful synchronous acquisition. Dropping a pending acquire
// future cannot decrement a different live producer's capacity.
struct SourceCredits {
    capacity: usize,
    current: Cell<usize>,
    changed: Repeated<()>,
}
struct ProducerCredit(Rc<SourceCredits>);
impl SourceCredits {
    async fn acquire(self: &Rc<Self>) -> ProducerCredit {
        loop {
            let changed = self.changed.listen();
            let current = self.current.get();
            if current < self.capacity {
                self.current.set(current + 1);
                return ProducerCredit(self.clone());
            }
            changed.await;
        }
    }
}
impl Drop for ProducerCredit {
    fn drop(&mut self) {
        let current = self.0.current.get();
        assert!(current > 0);
        self.0.current.set(current - 1);
        self.0.changed.notify();
    }
}
struct Waiter {
    notify: oneshot::Sender<Result<AtomicReceipt, SimError>>,
    finished: oneshot::Receiver<SimResult>,
    credit: ProducerCredit,
}
/// A real received response. The private ACK channel cannot be constructed
/// before the actual PortGet. Dropping it does not retire actor ownership.
///
/// ```compile_fail
/// use gwr_models::memory::atomic::AtomicReceipt;
/// fn forge(mut receipt:AtomicReceipt) {receipt.completion.control_value=Some(0);}
/// ```
/// ```compile_fail
/// use gwr_models::memory::atomic::SharedAtomicControl;
/// fn forge(handle:SharedAtomicControl) {handle.acknowledge(1).unwrap();}
/// ```
pub struct AtomicReceipt {
    completion: Completion,
    ack: Option<oneshot::Sender<()>>,
    finished: oneshot::Receiver<SimResult>,
    _credit: ProducerCredit,
}
impl AtomicReceipt {
    #[must_use]
    pub fn completion(&self) -> Completion {
        self.completion
    }
    pub async fn acknowledge(mut self) -> Result<Completion, SimError> {
        self.ack
            .take()
            .ok_or_else(|| error("typed receipt already acknowledged"))?
            .send(())
            .map_err(|_| error("typed receiver disappeared before ACK"))?;
        (&mut self.finished)
            .await
            .map_err(|_| error("typed publisher disappeared before actual ACK"))??;
        Ok(self.completion)
    }
}
/// Opaque dispatcher endpoint: static preflight is repeatable and effect-free.
/// Only `receive` mints a source ID; only an actual receipt can cause its ACK.
pub struct AtomicTargetSender {
    description: Rc<TargetDescription>,
    next_source: Cell<u64>,
    tx: RefCell<Option<OutPort<TypedInput>>>,
    issue_queue: RefCell<VecDeque<SourceJob>>,
    issue_changed: Repeated<()>,
    capacity: Rc<SourceCredits>,
    waiters: Rc<RefCell<BTreeMap<u64, Waiter>>>,
}
impl fmt::Debug for AtomicTargetSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AtomicTargetSender")
            .field("device", &self.description.device)
            .finish()
    }
}
impl AtomicTargetSender {
    pub fn validate_effect(&self, device: u64, address: u64, effect: Effect) -> SimResult {
        self.description.validate(device, address, effect)
    }
    pub fn validate_region(
        &self,
        device: u64,
        base: u64,
        bytes: u64,
        path: AtomicPath,
        config: &AtomicServiceConfig,
    ) -> SimResult {
        path.validate()?;
        if device != self.description.device
            || base != self.description.base
            || bytes.checked_sub(1).and_then(|n| base.checked_add(n)) != Some(self.description.last)
            || config != &self.description.config
        {
            return Err(error(
                "admitted region differs from actual shared memory target",
            ));
        }
        Ok(())
    }
    pub(crate) fn validate_ordinary_port_pair(
        &self,
        request: Option<&Rc<Entity>>,
        reply: Option<&Rc<Entity>>,
    ) -> SimResult {
        let expected_input = self.description.ordinary_input.borrow();
        let expected_output = self.description.ordinary_output.borrow();
        let matches = |actual: Option<&Rc<Entity>>, expected: Option<&Rc<Entity>>| matches!((actual,expected),(Some(a),Some(b)) if Rc::ptr_eq(a,b));
        if !matches(request, expected_input.as_ref()) || !matches(reply, expected_output.as_ref()) {
            return Err(error(
                "atomic issuer requires actual direct ordinary request AND reply port identities",
            ));
        }
        Ok(())
    }
    pub fn path(&self) -> AtomicPath {
        AtomicPath::CacheFreeDirect
    }
    pub fn device(&self) -> u64 {
        self.description.device
    }
    pub async fn receive(
        &self,
        device: u64,
        address: u64,
        effect: Effect,
    ) -> Result<AtomicReceipt, SimError> {
        self.receive_with_operand(device, address, effect, None)
            .await
    }
    pub(crate) async fn receive_with_operand(
        &self,
        device: u64,
        address: u64,
        effect: Effect,
        operand: Option<Rc<crate::processing_element::LocalOperandHandle>>,
    ) -> Result<AtomicReceipt, SimError> {
        self.validate_effect(device, address, effect)?;
        if let Some(local) = &operand {
            if !matches!(effect, Effect::Atomic { .. }) {
                return Err(error("local scratch operand requires atomic output"));
            }
            local.byte_offset()?;
        }
        let credit = self.capacity.acquire().await;
        let (notify, reply) = oneshot::channel();
        let (done, finished) = oneshot::channel();
        // Producer-owned queue is bounded by its own credits. Caller Drop
        // cannot drop/reuse a live PortPut: the internal issuer owns that
        // future until intake's actual finish_get. Queued caller cancellations
        // are discarded before minting a source ordinal or issuing a packet.
        self.issue_queue.borrow_mut().push_back(SourceJob {
            device,
            address,
            effect,
            operand,
            waiter: Waiter {
                notify,
                finished,
                credit,
            },
            done: Rc::new(RefCell::new(Some(done))),
        });
        self.issue_changed.notify();
        reply
            .await
            .map_err(|_| error("typed actual response unavailable"))?
    }
    pub async fn execute(
        &self,
        device: u64,
        address: u64,
        effect: Effect,
    ) -> Result<Completion, SimError> {
        self.receive(device, address, effect)
            .await?
            .acknowledge()
            .await
    }
}
struct SourceJob {
    device: u64,
    address: u64,
    effect: Effect,
    operand: Option<Rc<crate::processing_element::LocalOperandHandle>>,
    waiter: Waiter,
    done: Finished,
}
trait SharedControl {
    fn target(&self) -> Rc<AtomicTargetSender>;
    fn snapshot(&self) -> AtomicSnapshot;
    fn register(&self, group: ControlGroup) -> SimResult;
    fn cancel(&self, identity: u64) -> SimResult;
    fn finished(&self, invocation: u64, generation: u64) -> Result<bool, SimError>;
}
/// Observer/register/cancel authority only; no intake/commit/publication/ACK.
#[derive(Clone)]
pub struct SharedAtomicControl(Rc<dyn SharedControl>);
impl SharedAtomicControl {
    pub(crate) fn matches_target(&self, target: &Rc<AtomicTargetSender>) -> bool {
        Rc::ptr_eq(&self.0.target(), target)
    }
    #[must_use]
    pub fn snapshot(&self) -> AtomicSnapshot {
        self.0.snapshot()
    }
    pub fn register_control(&self, group: ControlGroup) -> SimResult {
        self.0.register(group)
    }
    pub fn cancel_owner(&self, owner: u64) -> SimResult {
        self.0.cancel(owner)
    }
    pub fn control_finished(&self, invocation: u64, generation: u64) -> Result<bool, SimError> {
        self.0.finished(invocation, generation)
    }
}
/// Optional bounded typed-owner history. Ordinary service has the same
/// authority and aggregate counters; millions of ordinary vector requests are
/// not copied into an unbounded witness. Defaults allocate no journal.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtomicOwnerWindow {
    pub owner: u64,
    pub source: u64,
    pub device: u64,
    pub address: u64,
    pub effect: Effect,
    pub accepted_tick: u64,
    pub deadline_tick: u64,
    pub committed_tick: Option<u64>,
    pub publication_tick: Option<u64>,
    pub ack_tick: Option<u64>,
    pub canceled_tick: Option<u64>,
    pub retired_tick: Option<u64>,
    pub committed_read_bytes: u64,
    pub committed_write_bytes: u64,
    pub control_value: Option<u32>,
}
struct Witness {
    limit: usize,
    owners: BTreeMap<u64, AtomicOwnerWindow>,
}
enum Envelope<T> {
    Ordinary(Option<T>),
    Typed {
        source: u64,
        finished: Finished,
        operand: Option<Rc<crate::processing_element::LocalOperandHandle>>,
    },
}
/// Private milestones and source→owner mapping. Neither ordinary tracker IDs
/// nor typed source IDs are service ordinals. Shared service is not exposed.
pub(crate) struct SharedMemoryAuthority<T: SimObject + AccessMemory> {
    service: Rc<AtomicService>,
    description: Rc<TargetDescription>,
    next_owner: Cell<u64>,
    clock: Clock,
    witness: RefCell<Option<Witness>>,
    envelopes: RefCell<BTreeMap<u64, Envelope<T>>>,
    ordinary_ready: RefCell<VecDeque<(u64, T)>>,
    typed_ready: RefCell<VecDeque<(u64, SharedReply, Finished)>>,
    ready_changed: Repeated<()>,
    typed_rx: RefCell<Option<InPort<TypedInput>>>,
    typed_tx: RefCell<Option<OutPort<SharedReply>>>,
    reply_rx: RefCell<Option<InPort<SharedReply>>>,
    sender: Rc<AtomicTargetSender>,
}
impl<T: SimObject + AccessMemory> SharedMemoryAuthority<T> {
    pub(crate) fn new(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        device: u64,
        base: u64,
        bytes: u64,
        config: &AtomicServiceConfig,
    ) -> Result<Rc<Self>, SimError> {
        config.validate()?;
        if usize::BITS < 64 {
            return Err(error(
                "shared atomic target requires the documented 64-bit counter runtime",
            ));
        }
        let service = Rc::new(AtomicService::new(device, base, bytes, config.clone())?);
        let description = Rc::new(TargetDescription {
            device,
            base,
            last: base
                .checked_add(bytes - 1)
                .ok_or_else(|| error("memory range overflow"))?,
            config: config.clone(),
            groups: RefCell::new(BTreeMap::new()),
            ordinary_input: RefCell::new(None),
            ordinary_output: RefCell::new(None),
        });
        let typed_rx = InPort::new(engine, clock, parent, "atomic_rx");
        let reply_rx = InPort::new(engine, clock, parent, "atomic_reply_rx");
        let mut tx = OutPort::new(parent, "atomic_issue");
        tx.connect(typed_rx.state())?;
        let mut typed_tx = OutPort::new(parent, "atomic_tx");
        typed_tx.connect(reply_rx.state())?;
        let waiters = Rc::new(RefCell::new(BTreeMap::new()));
        let sender = Rc::new(AtomicTargetSender {
            description: description.clone(),
            next_source: Cell::new(1),
            tx: RefCell::new(Some(tx)),
            issue_queue: RefCell::new(VecDeque::new()),
            issue_changed: Repeated::default(),
            capacity: Rc::new(SourceCredits {
                capacity: config.capacity,
                current: Cell::new(0),
                changed: Repeated::default(),
            }),
            waiters,
        });
        Ok(Rc::new(Self {
            service,
            description,
            clock: clock.clone(),
            witness: RefCell::new(None),
            next_owner: Cell::new(1),
            envelopes: RefCell::new(BTreeMap::new()),
            ordinary_ready: RefCell::new(VecDeque::new()),
            typed_ready: RefCell::new(VecDeque::new()),
            ready_changed: Repeated::default(),
            typed_rx: RefCell::new(Some(typed_rx)),
            typed_tx: RefCell::new(Some(typed_tx)),
            reply_rx: RefCell::new(Some(reply_rx)),
            sender,
        }))
    }
    pub(crate) fn bind_ordinary_input(&self, receiver: Rc<Entity>) {
        *self.description.ordinary_input.borrow_mut() = Some(receiver);
    }
    pub(crate) fn bind_ordinary_output(&self, receiver: Rc<Entity>) {
        *self.description.ordinary_output.borrow_mut() = Some(receiver);
    }
    pub(crate) fn control(self: &Rc<Self>) -> SharedAtomicControl {
        SharedAtomicControl(self.clone())
    }
    pub(crate) fn sender(&self) -> Rc<AtomicTargetSender> {
        self.sender.clone()
    }
    pub(crate) fn snapshot(&self) -> AtomicSnapshot {
        self.service.snapshot()
    }
    pub(crate) fn enable_witness(&self, limit: usize) -> SimResult {
        if limit == 0 || self.next_owner.get() != 1 || self.witness.borrow().is_some() {
            return Err(error(
                "typed witness requires positive explicit bound before intake",
            ));
        }
        *self.witness.borrow_mut() = Some(Witness {
            limit,
            owners: BTreeMap::new(),
        });
        Ok(())
    }
    pub(crate) fn owner_windows(&self) -> Option<Vec<AtomicOwnerWindow>> {
        self.witness
            .borrow()
            .as_ref()
            .map(|w| w.owners.values().cloned().collect())
    }
    fn window(&self, id: u64, f: impl FnOnce(&mut AtomicOwnerWindow)) {
        if let Some(w) = self.witness.borrow_mut().as_mut()
            && let Some(owner) = w.owners.get_mut(&id)
        {
            f(owner);
        }
    }

    pub(crate) fn register_control(&self, group: ControlGroup) -> SimResult {
        self.service.register_control(group.clone())?;
        self.description
            .groups
            .borrow_mut()
            .insert(group.invocation, group);
        Ok(())
    }
    pub(crate) fn cancel(&self, identity: u64) -> SimResult {
        self.service.cancel(identity)?;
        self.window(identity, |w| {
            w.canceled_tick = Some(self.clock.tick_now().tick());
        });
        if self.service.canceled_or_retired(identity)
            && let Some(Envelope::Typed {
                source, finished, ..
            }) = self.envelopes.borrow_mut().remove(&identity)
        {
            self.fail_typed(source, &finished, "typed request canceled before commit");
        }
        Ok(())
    }
    fn fail_typed(&self, source: u64, finished: &Finished, message: &str) {
        if let Some(waiter) = self.sender.waiters.borrow_mut().remove(&source) {
            let _ = waiter.notify.send(Err(error(message)));
        }
        if let Some(done) = finished.borrow_mut().take() {
            let _ = done.send(Err(error(message)));
        }
    }
    pub(crate) async fn wait_capacity(&self) {
        loop {
            let changed = self.service.changed.listen();
            if self.snapshot().current < self.service.capacity() as u64 {
                break;
            }
            changed.await;
        }
    }
    fn admit(&self, now: u64, mut request: Request, envelope: Envelope<T>) -> SimResult {
        let id = self.next_owner.get();
        let next = id
            .checked_add(1)
            .ok_or_else(|| error("shared memory owner identity exhausted"))?;
        request.identity = id;
        let source = match &envelope {
            Envelope::Typed { source, .. } => Some(*source),
            Envelope::Ordinary(_) => None,
        };
        if source.is_some()
            && self
                .witness
                .borrow()
                .as_ref()
                .is_some_and(|w| w.owners.len() >= w.limit)
        {
            return Err(error(
                "explicit typed witness bound exhausted before acceptance",
            ));
        }
        let observed = request.clone();
        let ticket = self.service.try_accept(now, request)?;
        if let Some(source) = source
            && let Some(w) = self.witness.borrow_mut().as_mut()
        {
            w.owners.insert(
                id,
                AtomicOwnerWindow {
                    owner: id,
                    source,
                    device: observed.device,
                    address: observed.address,
                    effect: observed.effect,
                    accepted_tick: now,
                    deadline_tick: ticket.deadline,
                    committed_tick: None,
                    publication_tick: None,
                    ack_tick: None,
                    canceled_tick: None,
                    retired_tick: None,
                    committed_read_bytes: 0,
                    committed_write_bytes: 0,
                    control_value: None,
                },
            );
        }

        self.envelopes.borrow_mut().insert(id, envelope);
        self.next_owner.set(next);
        Ok(())
    }
    pub(crate) fn accept_ordinary(
        &self,
        now: u64,
        address: u64,
        bytes: u64,
        kind: AccessType,
        response: Option<T>,
    ) -> SimResult {
        let effect = match kind {
            AccessType::ReadRequest => Effect::OrdinaryRead { bytes },
            AccessType::WriteRequest | AccessType::WriteNonPostedRequest => {
                Effect::OrdinaryWrite { bytes }
            }
            _ => return Err(error("unsupported shared ordinary request")),
        };
        if (kind == AccessType::WriteRequest) != response.is_none() {
            return Err(error("ordinary reply/posted kind mismatch"));
        }
        self.admit(
            now,
            Request {
                identity: 0,
                device: self.description.device,
                address,
                effect,
            },
            Envelope::Ordinary(response),
        )
    }
    pub(crate) async fn run_typed_issuer(self: Rc<Self>) -> SimResult {
        let mut tx = self
            .sender
            .tx
            .borrow_mut()
            .take()
            .ok_or_else(|| error("typed issuer already running"))?;
        loop {
            let changed = self.sender.issue_changed.listen();
            let next = self.sender.issue_queue.borrow_mut().pop_front();
            let Some(job) = next else {
                changed.await;
                continue;
            };
            if job.waiter.notify.is_canceled() {
                continue;
            }
            let identity = self.sender.next_source.get();
            let next = identity
                .checked_add(1)
                .ok_or_else(|| error("typed source identity exhausted"))?;
            let input = TypedInput {
                request: Request {
                    identity,
                    device: job.device,
                    address: job.address,
                    effect: job.effect,
                },
                finished: job.done,
                operand: job.operand,
            };
            // A disconnected port fails before registration; once polled the
            // actual PortPut remains worker-owned through ACK, even if the
            // external receive future disappears. No caller owns this future.
            let put = tx.put(input)?;
            self.sender
                .waiters
                .borrow_mut()
                .insert(identity, job.waiter);
            self.sender.next_source.set(next);
            put.await;
        }
    }
    pub(crate) async fn run_intake<F>(
        self: Rc<Self>,
        clock: Clock,
        mut ordinary: InPort<T>,
        response: F,
    ) -> SimResult
    where
        F: Fn(&T) -> Result<T, SimError>,
    {
        let mut typed = self
            .typed_rx
            .borrow_mut()
            .take()
            .ok_or_else(|| error("typed intake already running"))?;
        let mut next_source = 1_u64;
        // ONE receiver arbiter checks capacity, then polls both actual ports.
        // A losing PortStartGet has not consumed its value; its registered
        // waiter only causes a harmless wake. Alternate priority on ties.
        let mut prefer_typed = false;
        loop {
            self.wait_capacity().await;
            let choice = if prefer_typed {
                match select(typed.start_get()?, ordinary.start_get()?).await {
                    Either::Left((packet, _)) => Either::Right(packet),
                    Either::Right((packet, _)) => Either::Left(packet),
                }
            } else {
                match select(ordinary.start_get()?, typed.start_get()?).await {
                    Either::Left((packet, _)) => Either::Left(packet),
                    Either::Right((packet, _)) => Either::Right(packet),
                }
            };
            prefer_typed = !prefer_typed;
            match choice {
                Either::Left(access) => {
                    if access.dst_device().0 != self.description.device {
                        return Err(error("ordinary target memory device mismatch"));
                    }
                    let kind = access.access_type();
                    let reply = match kind {
                        AccessType::ReadRequest | AccessType::WriteNonPostedRequest => {
                            Some(response(&access)?)
                        }
                        AccessType::WriteRequest => None,
                        _ => return Err(error("unsupported shared ordinary access")),
                    };
                    self.accept_ordinary(
                        clock.tick_now().tick(),
                        access.dst_addr(),
                        u64::try_from(access.access_size_bytes())
                            .map_err(|e| SimError(e.to_string()))?,
                        kind,
                        reply,
                    )?;
                    ordinary.finish_get();
                }
                Either::Right(packet) => {
                    self.description.validate(
                        packet.request.device,
                        packet.request.address,
                        packet.request.effect,
                    )?;
                    if packet.request.identity != next_source {
                        return Err(error("typed source ordinal mismatch"));
                    }
                    let next = next_source
                        .checked_add(1)
                        .ok_or_else(|| error("typed source exhausted"))?;
                    let owner = self.next_owner.get();
                    self.admit(
                        clock.tick_now().tick(),
                        packet.request.clone(),
                        Envelope::Typed {
                            source: next_source,
                            finished: packet.finished.clone(),
                            operand: packet.operand.clone(),
                        },
                    )?;
                    let abandoned = self
                        .sender
                        .waiters
                        .borrow()
                        .get(&next_source)
                        .is_some_and(|w| w.notify.is_canceled());
                    if abandoned {
                        self.cancel(owner)?;
                    }
                    next_source = next;
                    typed.finish_get();
                }
            }
        }
    }
    pub(crate) async fn run_service(self: Rc<Self>, clock: Clock) -> SimResult {
        loop {
            let changed = self.service.changed.listen();
            let now = clock.tick_now().tick();
            let abandoned: Vec<_> = self
                .envelopes
                .borrow()
                .iter()
                .filter_map(|(id, e)| match e {
                    Envelope::Typed { source, .. }
                        if self
                            .sender
                            .waiters
                            .borrow()
                            .get(source)
                            .is_some_and(|w| w.notify.is_canceled()) =>
                    {
                        Some(*id)
                    }
                    _ => None,
                })
                .collect();
            for id in abandoned {
                self.cancel(id)?;
            }
            self.service.retire_canceled(now)?;
            if let Some(w) = self.witness.borrow_mut().as_mut() {
                for owner in w.owners.values_mut() {
                    if owner.canceled_tick.is_some()
                        && owner.committed_tick.is_none()
                        && owner.retired_tick.is_none()
                        && now >= owner.deadline_tick
                        && self.service.canceled_or_retired(owner.owner)
                    {
                        owner.retired_tick = Some(now);
                    }
                }
            }

            // Retired precommit owners no longer retain any response payload.
            self.envelopes
                .borrow_mut()
                .retain(|id, _| !self.service.canceled_or_retired(*id));
            let mut pending = self.service.pending();
            pending.sort_by_key(|t| (t.deadline, t.identity));
            let Some(next) = pending.first().copied() else {
                changed.await;
                continue;
            };
            if !self.service.canceled_or_retired(next.identity) {
                self.service.start_if_queued(next.identity)?;
            }
            if next.deadline > now {
                let timer = Box::pin(clock.wait_ticks(next.deadline - now)?);
                match select(timer, changed).await {
                    Either::Left(_) | Either::Right(_) => {}
                }
                continue;
            }
            if self.service.canceled_or_retired(next.identity) {
                continue;
            }
            let completion = self.service.commit(now, next.identity)?;
            self.window(next.identity, |w| {
                w.committed_tick = Some(now);
                w.committed_read_bytes = completion.read_bytes;
                w.committed_write_bytes = completion.write_bytes;
                w.control_value = completion.control_value;
            });
            let envelope = self
                .envelopes
                .borrow_mut()
                .remove(&next.identity)
                .ok_or_else(|| error("shared owner lacks actual envelope"))?;
            match envelope {
                Envelope::Ordinary(None) => self.service.acknowledge(next.identity)?, /* posted: no publication exists */
                Envelope::Ordinary(Some(reply)) => self
                    .ordinary_ready
                    .borrow_mut()
                    .push_back((next.identity, reply)),
                Envelope::Typed {
                    source,
                    finished,
                    operand,
                } => {
                    let mut reply = completion;
                    reply.identity = source;
                    self.typed_ready.borrow_mut().push_back((
                        next.identity,
                        SharedReply {
                            reply: AtomicReply { completion: reply },
                            owner: next.identity,
                            _operand: operand,
                        },
                        finished,
                    ));
                }
            }
            self.ready_changed.notify();
        }
    }
    pub(crate) async fn run_ordinary_publisher(self: Rc<Self>, mut tx: OutPort<T>) -> SimResult {
        loop {
            let changed = self.ready_changed.listen();
            let next = self.ordinary_ready.borrow_mut().pop_front();
            let Some((id, reply)) = next else {
                changed.await;
                continue;
            };
            if self.service.canceled_ready(id) {
                self.service.discard_canceled_response(id)?;
                continue;
            }
            self.service.begin_publication(id)?;
            tx.put(reply)?.await;
            self.service.acknowledge(id)?;
        }
    }
    pub(crate) async fn run_typed_publisher(self: Rc<Self>) -> SimResult {
        let mut tx = self
            .typed_tx
            .borrow_mut()
            .take()
            .ok_or_else(|| error("typed publisher already running"))?;
        loop {
            let changed = self.ready_changed.listen();
            let next = self.typed_ready.borrow_mut().pop_front();
            let Some((id, reply, finished)) = next else {
                changed.await;
                continue;
            };
            if self
                .sender
                .waiters
                .borrow()
                .get(&reply.reply.completion.identity)
                .is_some_and(|w| w.notify.is_canceled())
                && !self.service.canceled_ready(id)
            {
                self.cancel(id)?;
            }
            if self.service.canceled_ready(id) {
                self.service.discard_canceled_response(id)?;
                self.window(id, |w| w.retired_tick = Some(self.clock.tick_now().tick()));
                self.fail_typed(
                    reply.reply.completion.identity,
                    &finished,
                    "typed response canceled before publication",
                );
                continue;
            }
            self.service.begin_publication(id)?;
            self.window(id, |w| {
                w.publication_tick = Some(self.clock.tick_now().tick());
            });
            tx.put(reply)?.await;
            let canceled = self.service.canceled_publication(id);
            self.service.acknowledge(id)?;
            self.window(id, |w| {
                w.ack_tick = Some(self.clock.tick_now().tick());
                w.retired_tick = w.ack_tick;
            });
            if let Some(done) = finished.borrow_mut().take() {
                let outcome = if canceled {
                    Err(error(
                        "typed published response canceled; no continuation authority",
                    ))
                } else {
                    Ok(())
                };
                let _ = done.send(outcome);
            }
        }
    }
    pub(crate) async fn run_typed_reply_receiver(self: Rc<Self>) -> SimResult {
        let mut rx = self
            .reply_rx
            .borrow_mut()
            .take()
            .ok_or_else(|| error("typed reply receiver already running"))?;
        loop {
            let packet = rx.start_get()?.await;
            let waiter = self
                .sender
                .waiters
                .borrow_mut()
                .remove(&packet.reply.completion.identity)
                .ok_or_else(|| error("unsolicited/mismatched typed response"))?;
            let (ack, received) = oneshot::channel();
            let delivered = waiter
                .notify
                .send(Ok(AtomicReceipt {
                    completion: packet.reply.completion,
                    ack: Some(ack),
                    finished: waiter.finished,
                    _credit: waiter.credit,
                }))
                .is_ok();
            if !delivered || received.await.is_err() {
                // The PRIVATE receiver physically consumed this exact packet.
                // Abandonment cancels continuation authority, then its real
                // finish_get drains the live PortPut. No actor milestone is
                // exposed and irreversible effects/debt remain recorded.
                if !self.service.canceled_publication(packet.owner) {
                    self.cancel(packet.owner)?;
                }
            }
            rx.finish_get();
        }
    }
}

impl<T: SimObject + AccessMemory> SharedControl for SharedMemoryAuthority<T> {
    fn target(&self) -> Rc<AtomicTargetSender> {
        self.sender.clone()
    }
    fn snapshot(&self) -> AtomicSnapshot {
        self.snapshot()
    }
    fn register(&self, group: ControlGroup) -> SimResult {
        self.register_control(group)
    }
    fn cancel(&self, identity: u64) -> SimResult {
        self.cancel(identity)
    }
    fn finished(&self, invocation: u64, generation: u64) -> Result<bool, SimError> {
        self.service.control_finished(invocation, generation)
    }
}

#[cfg(test)]
mod tests {
    use gwr_engine::types::DeviceId;

    use super::*;
    use crate::memory::memory_access::MemoryAccess;
    use crate::memory::traits::ReadMemory;
    struct Empty;
    impl ReadMemory for Empty {
        fn read(&self) -> Vec<u8> {
            Vec::new()
        }
    }
    fn cfg() -> AtomicServiceConfig {
        AtomicServiceConfig {
            capacity: 2,
            bytes_per_tick: 64,
            delay_ticks: 0,
            bf16_pair_per_tick: Some(1),
            i32_fetch_add_per_tick: Some(1),
            lane_interleave_bytes: 4,
            lane_bytes_per_tick: vec![64],
        }
    }
    fn pair() -> Effect {
        Effect::Atomic {
            participant: 0,
            word: super::super::AtomicWord {
                device: 7,
                address: 4096,
                invocation: 99,
                generation: 1,
                operation: AtomicOperation::Bf16PairFadd,
                scope: super::super::AtomicScope::Agent,
                ordering: super::super::AtomicOrdering::Monotonic,
            },
        }
    }
    #[test]
    fn actual_caller_drop_before_port_get_cancels_then_drains_real_publication_and_later_issue() {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let authority = SharedMemoryAuthority::<MemoryAccess>::new(
            &engine,
            &clock,
            engine.top(),
            7,
            4096,
            256,
            &cfg(),
        )
        .unwrap();
        let ordinary = InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "ordinary");
        let mut idle = OutPort::new(engine.top(), "ordinary_source");
        idle.connect(ordinary.state()).unwrap();
        let mut ordinary_tx = OutPort::new(engine.top(), "ordinary_return");
        let ordinary_rx =
            InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "ordinary_sink");
        ordinary_tx.connect(ordinary_rx.state()).unwrap();
        let a = authority.clone();
        let c = clock.clone();
        engine
            .spawner()
            .spawn(async move { a.run_service(c).await });
        let a = authority.clone();
        engine
            .spawner()
            .spawn(async move { a.run_ordinary_publisher(ordinary_tx).await });
        let a = authority.clone();
        engine
            .spawner()
            .spawn(async move { a.run_typed_publisher().await });
        let a = authority.clone();
        engine
            .spawner()
            .spawn(async move { a.run_typed_issuer().await });
        let a = authority.clone();
        let c = clock.clone();
        engine.spawner().spawn(async move {
            a.run_intake(c, ordinary, |access| access.to_response(&Empty))
                .await
        });
        // Delay only the real receiver's start. The actual publisher owns its
        // PortPut and Publishing owner from tick1; no simulated ACK callback.
        let a = authority.clone();
        let c = clock.clone();
        engine.spawner().spawn(async move {
            c.wait_ticks(10)?.await;
            a.run_typed_reply_receiver().await
        });
        let a = authority.clone();
        let c = clock.clone();
        engine.spawner().spawn(async move {
            let receive = Box::pin(a.sender.receive(7, 4096, pair()));
            let stop = Box::pin(c.wait_ticks(3)?);
            match select(receive, stop).await {
                Either::Right(((), pending)) => drop(pending),
                Either::Left(_) => panic!("actual receiver has not started"),
            }
            let before = a.snapshot();
            assert_eq!(
                (before.current, before.committed_requests, before.completed),
                (1, 1, 0)
            );
            let reply = a.sender.execute(7, 4096, pair()).await?;
            assert_eq!(reply.identity, 2);
            assert!(c.tick_now().tick() >= 10);
            Ok(())
        });
        engine.run().unwrap();
        let s = authority.snapshot();
        assert_eq!(
            (
                s.current,
                s.completed,
                s.canceled_aftercommit,
                s.committed_read_bytes,
                s.committed_write_bytes
            ),
            (0, 1, 1, 8, 8)
        );
        // No ordinary request was fabricated to make the mixed component run.
        let _ = DeviceId(7);
    }
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep complete setup execution and assertions of this scenario together"
    )]
    fn actual_delivered_receipt_drop_never_authorizes_fetch_continuation() {
        for explicit_cancel in [false, true] {
            let mut engine = Engine::default();
            let clock = engine.default_clock();
            let memory = crate::memory::Memory::<MemoryAccess>::new_and_register(
                &engine,
                &clock,
                engine.top(),
                "m",
                crate::memory::MemoryConfig::new(4096, 256, 64, 0)
                    .with_arrival_aware_service(2)
                    .with_service_lanes(4, vec![64])
                    .with_atomic_service(7, cfg()),
            )
            .unwrap();
            let mut idle = OutPort::new(engine.top(), "idle");
            idle.connect(memory.port_rx()).unwrap();
            let rx = InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "unused");
            memory.connect_port_tx(rx.state()).unwrap();
            let control = memory.atomic_control().unwrap();
            control
                .register_control(ControlGroup {
                    invocation: 9,
                    generation: 1,
                    device: 7,
                    signal: 4224,
                    semaphore: 4228,
                    participants: 1,
                    publisher: 0,
                    output_words: vec![4096],
                    source_flags: super::super::SourceFlags::SplitKHgemmSc0Sc1,
                })
                .unwrap();
            let t = memory.atomic_target().unwrap();
            let c = clock.clone();
            let h = control.clone();
            engine.spawner().spawn(async move {
                t.execute(
                    7,
                    4096,
                    Effect::InitializeOutput {
                        invocation: 9,
                        generation: 1,
                    },
                )
                .await?;
                t.execute(
                    7,
                    4224,
                    Effect::Control {
                        invocation: 9,
                        generation: 1,
                        participant: 0,
                        effect: ControlEffect::PublishSignalOne,
                    },
                )
                .await?;
                let poll = t
                    .execute(
                        7,
                        4224,
                        Effect::Control {
                            invocation: 9,
                            generation: 1,
                            participant: 0,
                            effect: ControlEffect::PollSignal,
                        },
                    )
                    .await?;
                assert_eq!(poll.control_value, Some(1));
                let receipt = t
                    .receive(
                        7,
                        4228,
                        Effect::Control {
                            invocation: 9,
                            generation: 1,
                            participant: 0,
                            effect: ControlEffect::FetchAddOne,
                        },
                    )
                    .await?;
                assert_eq!(receipt.completion().control_value, Some(0));
                assert_eq!(h.snapshot().current, 1);
                if explicit_cancel {
                    h.cancel_owner(4)?;
                    assert_eq!(h.snapshot().canceled_aftercommit_outstanding, 1);
                }
                drop(receipt);
                c.wait_ticks(1)?.await;
                assert_eq!(
                    (
                        h.snapshot().current,
                        h.snapshot().completed,
                        h.snapshot().canceled_aftercommit
                    ),
                    (0, 3, 1)
                );
                assert!(!h.control_finished(9, 1)?);
                // Independent standalone word still works; canceled group
                // output is not claimed or synthetically
                // completed.
                let reply = t.execute(7, 4100, pair_at(4100)).await?;
                assert_eq!(reply.identity, 5);
                Ok(())
            });
            engine.run().unwrap();
            let s = control.snapshot();
            assert_eq!(
                (
                    s.current,
                    s.completed,
                    s.canceled_aftercommit,
                    s.i32_fetch_add_requests
                ),
                (0, 4, 1, 1)
            );
        }
    }
    #[test]
    fn canceled_pending_source_credit_cannot_release_another_live_producer() {
        let credits = Rc::new(SourceCredits {
            capacity: 1,
            current: Cell::new(0),
            changed: Repeated::default(),
        });
        futures::executor::block_on(async {
            let live = credits.acquire().await;
            assert_eq!(credits.current.get(), 1);
            let pending = Box::pin(credits.acquire());
            match select(pending, Box::pin(futures::future::ready(()))).await {
                Either::Right(((), pending)) => drop(pending),
                Either::Left(_) => panic!("capacity is genuinely held by the first producer"),
            }
            assert_eq!(credits.current.get(), 1);
            drop(live);
            assert_eq!(credits.current.get(), 0);
            let next = credits.acquire().await;
            assert_eq!(credits.current.get(), 1);
            drop(next);
            assert_eq!(credits.current.get(), 0);
        });
    }
    fn pair_at(address: u64) -> Effect {
        let mut effect = pair();
        if let Effect::Atomic { word, .. } = &mut effect {
            word.address = address;
        }
        effect
    }
}
