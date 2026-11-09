// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! One finite direct protocol; actual ACK owns credit, not a committed word.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::rc::Rc;

use async_trait::async_trait;
use futures::future::{Either, select};
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Event, Runnable, SimObject, TotalBytes};
use gwr_engine::types::{SimError, SimResult};
use gwr_track::Id;
use gwr_track::id::Unique;

use super::{
    AtomicPath, AtomicService, AtomicServiceConfig, AtomicSnapshot, Completion, ControlGroup,
    Request,
};

/// No Routable/AccessMemory implementation: ordinary caches/DMA cannot silently
/// interpret an atomic packet as a read/write. Native direct ports only.
#[derive(Clone, Debug)]
pub struct AtomicPacket(pub Request);
impl Unique for AtomicPacket {
    fn id(&self) -> Id {
        Id(self.0.identity)
    }
}
impl fmt::Display for AtomicPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "atomic request {} @{:x}",
            self.0.identity, self.0.address
        )
    }
}
impl TotalBytes for AtomicPacket {
    fn total_bytes(&self) -> usize {
        usize::try_from(self.0.demand().2).expect("validated operand width")
    }
}
impl SimObject for AtomicPacket {}
#[derive(Clone, Debug)]
pub struct AtomicReply {
    pub completion: Completion,
}
impl Unique for AtomicReply {
    fn id(&self) -> Id {
        Id(self.completion.identity)
    }
}
impl fmt::Display for AtomicReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "atomic response {}", self.completion.identity)
    }
}
impl TotalBytes for AtomicReply {
    fn total_bytes(&self) -> usize {
        usize::try_from(self.completion.response_bytes).expect("validated response width")
    }
}
impl SimObject for AtomicReply {}

/// Actor observer/cancellation authority. It cannot fabricate intake, commit,
/// publication or a consumer ACK. The inner service and tuple field are
/// private.
///
/// ```compile_fail
/// use gwr_models::memory::atomic::AtomicControlHandle;
/// fn forged_ack(handle: AtomicControlHandle) { handle.acknowledge(1).unwrap(); }
/// ```
/// ```compile_fail
/// use gwr_models::memory::atomic::AtomicControlHandle;
/// fn forged_commit(handle: AtomicControlHandle) { handle.commit(1, 1).unwrap(); }
/// ```
/// ```compile_fail
/// use gwr_models::memory::atomic::AtomicControlHandle;
/// fn forged_publication(handle: AtomicControlHandle) { handle.begin_publication(1).unwrap(); }
/// ```
/// ```compile_fail
/// use gwr_models::memory::atomic::AtomicControlHandle;
/// fn forged_start(handle: AtomicControlHandle) { handle.start(1).unwrap(); }
/// ```
/// ```compile_fail
/// use gwr_models::memory::atomic::{AtomicControlHandle, Request};
/// fn forged_intake(handle: AtomicControlHandle, request: Request) { handle.try_accept(0, request).unwrap(); }
/// ```
/// ```compile_fail
/// use gwr_models::memory::atomic::AtomicMemory;
/// fn exposed_service(memory: &AtomicMemory) { let _ = &memory.service; }
/// ```
#[derive(Clone)]
pub struct AtomicControlHandle(Rc<AtomicService>);
impl AtomicControlHandle {
    pub fn snapshot(&self) -> AtomicSnapshot {
        self.0.snapshot()
    }
    pub fn register_control(&self, group: ControlGroup) -> SimResult {
        self.0.register_control(group)
    }
    pub fn cancel(&self, identity: u64) -> SimResult {
        self.0.cancel(identity)
    }
    pub fn control_finished(&self, invocation: u64, generation: u64) -> Result<bool, SimError> {
        self.0.control_finished(invocation, generation)
    }
}
pub struct AtomicMemory {
    service: Rc<AtomicService>,
    clock: Clock,
    rx: RefCell<Option<InPort<AtomicPacket>>>,
    tx: RefCell<Option<OutPort<AtomicReply>>>,
    ready: Rc<RefCell<VecDeque<AtomicReply>>>,
    ready_changed: Repeated<()>,
    spawner: gwr_engine::executor::Spawner,
}
impl AtomicMemory {
    pub fn control_handle(&self) -> AtomicControlHandle {
        AtomicControlHandle(self.service.clone())
    }
    pub fn ready_response_count(&self) -> usize {
        self.ready.borrow().len()
    }
    /// Registration is explicit; no existing Memory/vendor preset is replaced.
    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        device: u64,
        base: u64,
        bytes: u64,
        config: AtomicServiceConfig,
        path: AtomicPath,
    ) -> Result<Rc<Self>, SimError> {
        path.validate()?;
        let capacity = config.capacity;
        let service = Rc::new(AtomicService::new(device, base, bytes, config)?);
        let mut ready = VecDeque::new();
        ready
            .try_reserve_exact(capacity)
            .map_err(|e| SimError(e.to_string()))?;
        let component = Rc::new(Self {
            service,
            clock: clock.clone(),
            rx: RefCell::new(Some(InPort::new(
                engine,
                clock,
                engine.top(),
                "atomic_memory_rx",
            ))),
            tx: RefCell::new(Some(OutPort::new(engine.top(), "atomic_memory_tx"))),
            ready: Rc::new(RefCell::new(ready)),
            ready_changed: Repeated::default(),
            spawner: engine.spawner(),
        });
        engine.register(component.clone());
        Ok(component)
    }
    pub fn port_rx(&self) -> PortStateResult<AtomicPacket> {
        self.rx
            .borrow()
            .as_ref()
            .ok_or_else(|| SimError("atomic intake already running".into()))?
            .state()
    }
    pub fn connect_port_tx(&self, state: PortStateResult<AtomicReply>) -> SimResult {
        self.tx
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| SimError("atomic publisher already running".into()))?
            .connect(state)
    }
}
#[async_trait(?Send)]
impl Runnable for AtomicMemory {
    async fn run(&self) -> SimResult {
        let mut rx = self
            .rx
            .borrow_mut()
            .take()
            .ok_or_else(|| SimError("atomic intake already running".into()))?;
        let tx = self
            .tx
            .borrow_mut()
            .take()
            .ok_or_else(|| SimError("atomic publisher already running".into()))?;
        let service = self.service.clone();
        let clock = self.clock.clone();
        let ready = self.ready.clone();
        let wake = self.ready_changed.clone();
        self.spawner
            .spawn(async move { run_service(service, clock, ready, wake).await });
        let service = self.service.clone();
        let ready = self.ready.clone();
        let wake = self.ready_changed.clone();
        self.spawner
            .spawn(async move { run_publisher(service, ready, wake, tx).await });
        loop {
            let changed = self.service.changed.listen();
            if self.service.snapshot().current >= self.service_capacity() {
                changed.await;
                continue;
            }
            let packet = rx.start_get()?.await;
            self.service
                .try_accept(self.clock.tick_now().tick(), packet.0)?;
            rx.finish_get();
        }
    }
}
impl AtomicMemory {
    fn service_capacity(&self) -> u64 {
        self.service.capacity() as u64
    }
}
async fn run_service(
    service: Rc<AtomicService>,
    clock: Clock,
    ready: Rc<RefCell<VecDeque<AtomicReply>>>,
    wake: Repeated<()>,
) -> SimResult {
    loop {
        let changed = service.changed.listen();
        let now = clock.tick_now().tick();
        service.retire_canceled(now)?;
        let mut pending = service.pending();
        pending.sort_by_key(|t| (t.deadline, t.identity));
        if pending.is_empty() {
            changed.await;
            continue;
        }
        let next = pending[0];
        if !service.canceled_or_retired(next.identity) {
            service.start_if_queued(next.identity)?;
        }
        if next.deadline > now {
            let timer = Box::pin(clock.wait_ticks(next.deadline - now)?);
            match select(timer, changed).await {
                Either::Left(_) | Either::Right(_) => {}
            }
            continue;
        }
        if service.canceled_or_retired(next.identity) {
            continue;
        }
        let completion = service.commit(now, next.identity)?;
        ready.borrow_mut().push_back(AtomicReply { completion });
        wake.notify();
    }
}
async fn run_publisher(
    service: Rc<AtomicService>,
    ready: Rc<RefCell<VecDeque<AtomicReply>>>,
    wake: Repeated<()>,
    mut tx: OutPort<AtomicReply>,
) -> SimResult {
    loop {
        let changed = wake.listen();
        let reply = ready.borrow_mut().pop_front();
        let Some(reply) = reply else {
            changed.await;
            continue;
        };
        let id = reply.completion.identity;
        if service.canceled_ready(id) {
            service.discard_canceled_response(id)?;
            continue;
        }
        if service.canceled_or_retired(id) {
            continue;
        }
        service.begin_publication(id)?;
        tx.put(reply)?.await;
        service.acknowledge(id)?;
    }
}
