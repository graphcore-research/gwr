// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! A basic n-way set-associative cache model.
//!
//! The cache provides no memory ordering guarantees.
//!
//! TODO: Should cache accesses return an error if they are not
//! cache-line aligned or sized?
//!
//! ```text
//!  ----------------------------
//!  |          DEVICE          |
//!  ----------------------------
//!       |               |
//!       |               |
//!  ----------------------------
//!  |  dev_rx          dev_tx  |
//!  |    |               ^     |
//!  |    |               |     |
//!  |    |             delay   |
//!  |    |     CACHE     |     |
//!  |    |           response  |
//!  |    +---------> arbiter   |
//!  |    |               ^     |
//!  |    v               |     |
//!  | request            |     |
//!  | arbiter <----------|     |
//!  |    |               |     |
//!  |  delay             |     |
//!  |    |               |     |
//!  |    v               |     |
//!  |  mem_tx         mem_rx   |
//!  ----------------------------
//!       |               |
//!       |               |
//!  ----------------------------
//!  |         MEM/BUS          |
//!  ----------------------------
//! ```

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt::{self, Display};
use std::rc::Rc;

use async_trait::async_trait;
use futures::{FutureExt, select_biased};
use gwr_components::arbiter::Arbiter;
use gwr_components::arbiter::policy::{Priority, PriorityRoundRobin, RoundRobin};
use gwr_components::delay::Delay;
use gwr_components::{connect_tx, port_rx, take_option};
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::executor::Spawner;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::time::compute_adjusted_value_and_rate;
use gwr_engine::traits::{Event, Runnable, SimObject};
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;
use gwr_track::{build_aka, debug, trace};

pub mod coherency_manager;
mod config;
mod contents;
mod line_state;
mod metrics;
mod rx_handler_state;
pub mod traits;

pub use config::CacheConfig;
use contents::{AllocateResult, CacheContents};
use line_state::{AllocateExclusive, AllocateShared, GrantShared, LineState, LineStateTransition};
use metrics::CacheMetrics;
use rx_handler_state::{RetryRequest, RxHandlingState};

use crate::cache::coherency_manager::{CoherenceOp, CoherenceState};
use crate::cache::traits::CoherentAccess;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum CacheHintType {
    Allocate,
    NoAllocate,
}

pub struct CacheStatsDisplay {
    prefix: String,
    time_now_ns: f64,
    payload_bytes_read: usize,
    payload_bytes_written: usize,
    num_hits: usize,
    num_misses: usize,
}

impl CacheStatsDisplay {
    #[must_use]
    pub fn new(
        prefix: impl Into<String>,
        time_now_ns: f64,
        payload_bytes_read: usize,
        payload_bytes_written: usize,
        num_hits: usize,
        num_misses: usize,
    ) -> Self {
        Self {
            prefix: prefix.into(),
            time_now_ns,
            payload_bytes_read,
            payload_bytes_written,
            num_hits,
            num_misses,
        }
    }
}

impl Display for CacheStatsDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (read_value, read_per_second) =
            compute_adjusted_value_and_rate(self.time_now_ns, self.payload_bytes_read);
        let (write_value, write_per_second) =
            compute_adjusted_value_and_rate(self.time_now_ns, self.payload_bytes_written);
        let num_accesses = self.num_hits + self.num_misses;
        let hit_rate = if num_accesses == 0 {
            0.0
        } else {
            self.num_hits as f64 / num_accesses as f64 * 100.0
        };

        writeln!(f, "{}:", self.prefix)?;
        writeln!(
            f,
            "  Payload read: {} bytes, {read_value:.2}, {read_per_second:.2}/s",
            self.payload_bytes_read
        )?;
        writeln!(
            f,
            "  Payload written: {} bytes, {write_value:.2}, {write_per_second:.2}/s",
            self.payload_bytes_written
        )?;
        write!(
            f,
            "  Hits: {}, misses: {}, hit rate: {hit_rate:.2}%",
            self.num_hits, self.num_misses
        )
    }
}

#[derive(EntityGet, EntityDisplay)]
pub struct Cache<T>
where
    T: SimObject + CoherentAccess,
{
    entity: Rc<Entity>,

    clock: Clock,
    spawner: Spawner,
    metrics: Rc<CacheMetrics>,
    contents: Rc<RefCell<CacheContents<T>>>,

    response_delay: RefCell<Option<Rc<Delay<T>>>>,
    request_delay: RefCell<Option<Rc<Delay<T>>>>,

    dev_rx: RefCell<Option<InPort<T>>>,
    mem_rx: RefCell<Option<InPort<T>>>,

    // Internal ports
    dev_rx_mem_ack_arb: RefCell<Option<OutPort<T>>>,
    mem_rx_mem_ack_arb: RefCell<Option<OutPort<T>>>,
    dev_rx_to_mem_arb: RefCell<Option<OutPort<T>>>,
    mem_rx_to_mem_arb: RefCell<Option<OutPort<T>>>,
    rsp_arb_0: RefCell<Option<OutPort<T>>>,
    rsp_arb_1: RefCell<Option<OutPort<T>>>,
}

struct RequestArbiterPorts<T>
where
    T: SimObject + CoherentAccess,
{
    dev_rx_mem_ack_arb: OutPort<T>,
    mem_rx_mem_ack_arb: OutPort<T>,
    dev_rx_to_mem_arb: OutPort<T>,
    mem_rx_to_mem_arb: OutPort<T>,
}

struct ResponseArbiterPorts<T>
where
    T: SimObject + CoherentAccess,
{
    rsp_arb_0: OutPort<T>,
    rsp_arb_1: OutPort<T>,
}

impl<T> Cache<T>
where
    T: SimObject + CoherentAccess,
{
    /// Create an instance of the cache and register it with the Engine.
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        config: CacheConfig,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));

        let response_delay_aka = build_aka!(aka, &entity, &[("dev_tx", "tx")]);
        let response_delay = Delay::new_and_register_with_renames(
            engine,
            clock,
            &entity,
            "rsp_delay",
            Some(&response_delay_aka),
            config.delay_ticks,
        );

        let request_delay_aka = build_aka!(aka, &entity, &[("mem_tx", "tx")]);
        let request_delay = Delay::new_and_register_with_renames(
            engine,
            clock,
            &entity,
            "req_delay",
            Some(&request_delay_aka),
            config.delay_ticks,
        );

        let request_arbiter_ports =
            Self::build_request_arbiter(engine, clock, &entity, &request_delay);
        let response_arbiter_ports =
            Self::build_response_arbiter(engine, clock, &entity, &response_delay);

        let dev_rx = InPort::new_with_renames(engine, clock, &entity, "dev_rx", aka);
        let mem_rx = InPort::new_with_renames(engine, clock, &entity, "mem_rx", aka);

        let spawner = engine.spawner();
        let rc_self = Rc::new(Self {
            entity,
            clock: clock.clone(),
            spawner,
            metrics: Rc::new(CacheMetrics::default()),
            contents: Rc::new(RefCell::new(CacheContents::new(config))),
            response_delay: RefCell::new(Some(response_delay)),
            request_delay: RefCell::new(Some(request_delay)),
            dev_rx: RefCell::new(Some(dev_rx)),
            mem_rx: RefCell::new(Some(mem_rx)),

            dev_rx_mem_ack_arb: RefCell::new(Some(request_arbiter_ports.dev_rx_mem_ack_arb)),
            mem_rx_mem_ack_arb: RefCell::new(Some(request_arbiter_ports.mem_rx_mem_ack_arb)),
            dev_rx_to_mem_arb: RefCell::new(Some(request_arbiter_ports.dev_rx_to_mem_arb)),
            mem_rx_to_mem_arb: RefCell::new(Some(request_arbiter_ports.mem_rx_to_mem_arb)),
            rsp_arb_0: RefCell::new(Some(response_arbiter_ports.rsp_arb_0)),
            rsp_arb_1: RefCell::new(Some(response_arbiter_ports.rsp_arb_1)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    fn build_request_arbiter(
        engine: &Engine,
        clock: &Clock,
        entity: &Rc<Entity>,
        request_delay: &Rc<Delay<T>>,
    ) -> RequestArbiterPorts<T> {
        let policy = Box::new(
            PriorityRoundRobin::from_priorities(
                vec![Priority::High, Priority::High, Priority::Low, Priority::Low],
                4,
            )
            .expect("Internal priorities should match arbiter inputs"),
        );
        let request_arbiter =
            Arbiter::new_and_register(engine, clock, entity, "req_arb", 4, policy);
        request_arbiter
            .connect_port_tx(request_delay.port_rx())
            .expect("Internal ports should connect without error");

        let mut dev_rx_mem_ack_arb = OutPort::new(entity, "dev_rx_mem_ack_arb");
        dev_rx_mem_ack_arb
            .connect(request_arbiter.port_rx_i(0))
            .expect("Internal ports should connect without error");

        let mut mem_rx_mem_ack_arb = OutPort::new(entity, "mem_rx_mem_ack_arb");
        mem_rx_mem_ack_arb
            .connect(request_arbiter.port_rx_i(1))
            .expect("Internal ports should connect without error");

        let mut dev_rx_to_mem_arb = OutPort::new(entity, "dev_rx_to_mem_arb");
        dev_rx_to_mem_arb
            .connect(request_arbiter.port_rx_i(2))
            .expect("Internal ports should connect without error");

        let mut mem_rx_to_mem_arb = OutPort::new(entity, "mem_rx_to_mem_arb");
        mem_rx_to_mem_arb
            .connect(request_arbiter.port_rx_i(3))
            .expect("Internal ports should connect without error");

        RequestArbiterPorts {
            dev_rx_mem_ack_arb,
            mem_rx_mem_ack_arb,
            dev_rx_to_mem_arb,
            mem_rx_to_mem_arb,
        }
    }

    fn build_response_arbiter(
        engine: &Engine,
        clock: &Clock,
        entity: &Rc<Entity>,
        response_delay: &Rc<Delay<T>>,
    ) -> ResponseArbiterPorts<T> {
        let policy = Box::new(RoundRobin::new());
        let response_arbiter =
            Arbiter::new_and_register(engine, clock, entity, "rsp_arb", 2, policy);
        response_arbiter
            .connect_port_tx(response_delay.port_rx())
            .expect("Internal ports should connect without error");

        let mut rsp_arb_0 = OutPort::new(entity, "rsp_arb_0");
        rsp_arb_0
            .connect(response_arbiter.port_rx_i(0))
            .expect("Internal ports should connect without error");

        let mut rsp_arb_1 = OutPort::new(entity, "rsp_arb_1");
        rsp_arb_1
            .connect(response_arbiter.port_rx_i(1))
            .expect("Internal ports should connect without error");

        ResponseArbiterPorts {
            rsp_arb_0,
            rsp_arb_1,
        }
    }

    /// Create an instance of the cache and register it with the Engine.
    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        config: CacheConfig,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(engine, clock, parent, name, None, config)
    }

    pub fn connect_port_dev_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        connect_tx!(self.response_delay, connect_port_tx ; port_state)
    }

    pub fn connect_port_mem_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        connect_tx!(self.request_delay, connect_port_tx ; port_state)
    }

    pub fn port_dev_rx(&self) -> PortStateResult<T> {
        port_rx!(self.dev_rx, state)
    }

    pub fn port_mem_rx(&self) -> PortStateResult<T> {
        port_rx!(self.mem_rx, state)
    }
    #[must_use]
    pub fn payload_bytes_read(&self) -> usize {
        self.metrics.payload_bytes_read()
    }

    #[must_use]
    pub fn payload_bytes_written(&self) -> usize {
        self.metrics.payload_bytes_written()
    }

    #[must_use]
    pub fn num_hits(&self) -> usize {
        self.metrics.num_hits()
    }

    #[must_use]
    pub fn num_misses(&self) -> usize {
        self.metrics.num_misses()
    }

    pub fn dump_stats(&self, time_now_ns: f64) {
        self.metrics.dump_stats(&self.entity, time_now_ns);
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Cache<T>
where
    T: SimObject + CoherentAccess,
{
    async fn run(&self) -> SimResult {
        let retry_requests = Rc::new(RefCell::new(VecDeque::new()));
        let retry_changed = Repeated::default();

        {
            // Spawn a task to handle requests from the device side
            let state = RxHandlingState::new(
                self.entity.clone(),
                take_option!(self.dev_rx),
                self.clock.clone(),
                self.contents.clone(),
                self.metrics.clone(),
                retry_requests.clone(),
                retry_changed.clone(),
            );
            let rsp_arb_1 = take_option!(self.rsp_arb_1);
            let dev_to_mem = take_option!(self.dev_rx_to_mem_arb);
            let mem_ack = take_option!(self.dev_rx_mem_ack_arb);
            self.spawner
                .spawn(async move { run_dev_rx(state, dev_to_mem, mem_ack, rsp_arb_1).await });
        }

        {
            // Spawn a task to handle responses from the memory side
            let state = RxHandlingState::new(
                self.entity.clone(),
                take_option!(self.mem_rx),
                self.clock.clone(),
                self.contents.clone(),
                self.metrics.clone(),
                retry_requests.clone(),
                retry_changed.clone(),
            );
            let rsp_arb_0 = take_option!(self.rsp_arb_0);
            let dev_to_mem = take_option!(self.mem_rx_to_mem_arb);
            let mem_ack = take_option!(self.mem_rx_mem_ack_arb);
            self.spawner
                .spawn(async move { run_mem_rx(state, dev_to_mem, mem_ack, rsp_arb_0).await });
        }

        Ok(())
    }
}

// Device request path.

async fn run_dev_rx<T>(
    mut state: RxHandlingState<T>,
    mut dev_to_mem: OutPort<T>,
    mut mem_ack: OutPort<T>,
    mut rsp_arb: OutPort<T>,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    let retry_changed = state.retry_changed_event();
    loop {
        if let Some(request) = state.take_next_retry_request() {
            handle_retry_request(&state, &mut dev_to_mem, &mut mem_ack, &mut rsp_arb, request)
                .await?;
            try_advance_barrier(&state, &mut dev_to_mem, &mut rsp_arb).await?;
            continue;
        }

        let request = select_biased! {
            _ = retry_changed.listen().fuse() => {
                continue;
            }
            request = state.rx.get()?.fuse() => request,
        };
        trace!(state.entity ; "Device request {request}");
        let total_bytes = request.total_bytes();
        state.record_received_coherence_op(request.coherence_op());
        handle_request(
            &state,
            &mut dev_to_mem,
            &mut mem_ack,
            &mut rsp_arb,
            request,
            true,
        )
        .await?;
        let ticks = total_bytes.div_ceil(state.bw_bytes_per_cycle);
        state.clock.wait_ticks(ticks as u64).await;
    }
}

async fn handle_retry_request<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    mem_ack: &mut OutPort<T>,
    rsp: &mut OutPort<T>,
    retry: RetryRequest<T>,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    match retry {
        RetryRequest::Device(request) => {
            if state.contents.borrow().has_active_barrier() {
                debug!(state.entity ; "Queue access {} behind barrier", request.id());
                state
                    .contents
                    .borrow_mut()
                    .queue_blocked_device_request(request);
                return Ok(());
            }

            handle_request(state, dev_to_mem, mem_ack, rsp, request, true).await
        }
        RetryRequest::Pending(request) => {
            handle_request(state, dev_to_mem, mem_ack, rsp, request, false).await
        }
    }
}

async fn handle_request<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    mem_ack: &mut OutPort<T>,
    rsp_arb: &mut OutPort<T>,
    request: T,
    count_metrics: bool,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    if count_metrics && state.contents.borrow().has_active_barrier() {
        debug!(state.entity ; "Queue access {} behind barrier", request.id());
        state
            .contents
            .borrow_mut()
            .queue_blocked_device_request(request);
        return Ok(());
    }

    let access_type = request.access_type();
    match access_type {
        AccessType::Control => {
            handle_control_request(state, dev_to_mem, mem_ack, rsp_arb, request).await?;
        }
        AccessType::ReadRequest => {
            handle_read_request(state, dev_to_mem, mem_ack, rsp_arb, request, count_metrics)
                .await?;
        }
        AccessType::BarrierRequest => {
            handle_barrier_request(state, dev_to_mem, rsp_arb, request).await?;
        }
        AccessType::WriteRequest | AccessType::WriteNonPostedRequest => {
            handle_write_request(state, dev_to_mem, mem_ack, rsp_arb, request, count_metrics)
                .await?;
        }

        AccessType::ReadResponse
        | AccessType::WriteNonPostedResponse
        | AccessType::BarrierResponse => {
            return sim_error!(
                "{}: unsupported AccessType from device: {access_type}",
                state.entity
            );
        }
    }

    Ok(())
}

// Barrier and pending-request replay path.

async fn handle_barrier_request<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    rsp: &mut OutPort<T>,
    request: T,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    if state.contents.borrow().has_active_barrier() {
        debug!(state.entity ; "Queue barrier access {} behind active barrier", request.id());
        state
            .contents
            .borrow_mut()
            .queue_blocked_device_request(request);
        return Ok(());
    }

    debug!(state.entity ; "Start barrier access {}", request.id());
    state.contents.borrow_mut().set_active_barrier(request);
    try_advance_barrier(state, dev_to_mem, rsp).await
}

async fn try_advance_barrier<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    rsp: &mut OutPort<T>,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    let barrier = {
        let contents = state.contents.borrow();
        if !contents.has_active_barrier()
            || contents.has_outstanding_work()
            || state.has_retry_requests()
        {
            return Ok(());
        }
        contents.active_barrier()
    };
    let Some(barrier) = barrier else {
        return Ok(());
    };

    if state.contents.borrow().barrier_forwarded() {
        return Ok(());
    }

    if state.contents.borrow().is_coherent() {
        let forwarded = state.rewrite_request_source(&barrier, None)?;
        debug!(state.entity ; "Forward barrier access {} as {}", barrier.id(), forwarded);
        state.contents.borrow_mut().mark_barrier_forwarded();
        dev_to_mem.put(forwarded)?.await;
        return Ok(());
    }

    debug!(state.entity ; "Complete local barrier access {}", barrier.id());
    complete_barrier(state, rsp, barrier).await
}

async fn complete_forwarded_barrier<T>(
    state: &RxHandlingState<T>,
    rsp: &mut OutPort<T>,
    access: T,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    let Some(barrier) = state.contents.borrow_mut().active_barrier() else {
        return sim_error!(
            "{}: received barrier response without active barrier",
            state.entity
        );
    };
    debug!(state.entity ; "Complete forwarded barrier access {} with {}", barrier.id(), access);
    complete_barrier(state, rsp, barrier).await
}

async fn complete_barrier<T>(
    state: &RxHandlingState<T>,
    rsp: &mut OutPort<T>,
    barrier: T,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    let response = barrier.to_response(state.contents.as_ref())?;
    rsp.put(response)?.await;
    state.contents.borrow_mut().clear_active_barrier();
    queue_blocked_device_requests_for_retry(state);
    Ok(())
}

fn queue_blocked_device_requests_for_retry<T>(state: &RxHandlingState<T>)
where
    T: SimObject + CoherentAccess,
{
    while let Some(request) = state
        .contents
        .borrow_mut()
        .take_next_blocked_device_request()
    {
        debug!(state.entity ; "Replay blocked device access {} {}", request.id(), request.access_type());
        state.queue_device_retry_request(request);
    }
}

async fn process_pending_line_waiters_for_line<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    rsp: &mut OutPort<T>,
    line_access: &T,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    let pending = state
        .contents
        .borrow_mut()
        .take_pending_line_waiters(line_access);
    if let Some(pending) = pending {
        debug!(
            state.entity ;
            "Wake {} pending request(s) after access {} for line addr 0x{:x}",
            pending.len(),
            line_access.id(),
            line_access.dst_addr()
        );
        for request in pending {
            state.queue_pending_retry_request(request);
        }
    }

    let (line_tag, _) = state
        .contents
        .borrow()
        .tag_and_set_index_for_addr(line_access.dst_addr());
    let same_set_pending = state
        .contents
        .borrow_mut()
        .take_pending_line_waiters_for_index_except_tag(line_access.dst_addr(), line_tag);
    if let Some(same_set_pending) = same_set_pending {
        debug!(
            state.entity ;
            "Retry {} pending request(s) for set of line addr 0x{:x}",
            same_set_pending.len(),
            line_access.dst_addr()
        );
        for request in same_set_pending {
            state.queue_pending_retry_request(request);
        }
    }
    try_advance_barrier(state, dev_to_mem, rsp).await
}

async fn handle_invalidate<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    mem_ack: &mut OutPort<T>,
    rsp: &mut OutPort<T>,
    access: &T,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    if let Some(ack) = invalidate_line_and_make_ack(state, mem_ack, access).await? {
        mem_ack.put(ack)?.await;
    }
    process_pending_line_waiters_for_line(state, dev_to_mem, rsp, access).await
}

async fn handle_device_invalidate<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    mem_ack: &mut OutPort<T>,
    rsp: &mut OutPort<T>,
    access: &T,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    if let Some(ack) = invalidate_line_and_make_ack(state, mem_ack, access).await? {
        rsp.put(ack)?.await;
    }
    process_pending_line_waiters_for_line(state, dev_to_mem, rsp, access).await
}

async fn invalidate_line_and_make_ack<T>(
    state: &RxHandlingState<T>,
    mem_ack: &mut OutPort<T>,
    access: &T,
) -> Result<Option<T>, SimError>
where
    T: SimObject + CoherentAccess,
{
    let addr = access.dst_addr();
    debug!(
        state.entity ;
        "Received invalidate access {} for line 0x{addr:x}",
        access.id()
    );
    if state.contents.borrow().is_modified(addr) {
        state
            .queue_dirty_line_writeback(mem_ack, access, addr)
            .await?;
    }
    state.contents.borrow_mut().invalidate(addr);

    let cache_device_id = state.contents.borrow().device_id();
    let ack = access
        .clone()
        .with_routing(access.src_device(), cache_device_id)
        .with_coherence_op(Some(CoherenceOp::InvalidateAck));

    log_cache_forward(&state.entity, "Send invalidate ack", access, &ack);

    state.record_sent_coherence_op(ack.coherence_op());
    Ok(Some(ack))
}

// Device-side access handlers.

async fn handle_control_request<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    mem_ack: &mut OutPort<T>,
    rsp_arb_1: &mut OutPort<T>,
    request: T,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    match request.coherence_op() {
        Some(CoherenceOp::Invalidate) => {
            handle_device_invalidate(state, dev_to_mem, mem_ack, rsp_arb_1, &request).await?;
        }
        _ => {
            let addr = request.dst_addr();
            return sim_error!(
                "{}: unsupported coherence op {:?} on device control access {} for line 0x{addr:x}",
                state.entity,
                request.coherence_op(),
                request.id(),
            );
        }
    }
    Ok(())
}

async fn handle_read_request<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    mem_ack: &mut OutPort<T>,
    rsp_arb_1: &mut OutPort<T>,
    request: T,
    count_metrics: bool,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    if count_metrics {
        state
            .metrics
            .record_payload_bytes_read(request.access_size_bytes());
    }
    let addr = request.dst_addr();
    let line_state = state.contents.borrow().state_for(addr);
    match line_state {
        Some(x) if x.can_read_hit() => {
            if count_metrics {
                state.metrics.record_read_hit();
            }

            debug!(
                state.entity ;
                "{} hit for line 0x{addr:x} in state {}",
                request.id(),
                x.as_str()
            );
            let response = request.to_response(state.contents.as_ref())?;
            rsp_arb_1.put(response)?.await;
        }
        Some(x) if x.is_allocated() => {
            if count_metrics {
                state.metrics.record_read_pending_hit();
            }

            debug!(
                state.entity ;
                "{} pending hit for line 0x{addr:x}; queued behind allocation in state {}",
                request.id(),
                x.as_str()
            );
            state.contents.borrow_mut().add_pending_line_waiter(request);
        }
        None => {
            if count_metrics {
                state.metrics.record_read_miss();
            }

            let forwarded = state.prepare_miss_request(&request, CoherenceState::Shared)?;
            if !should_allocate(&request) {
                debug!(state.entity ; "{} miss for line 0x{addr:x}; forward without allocation", request.id());
                state
                    .contents
                    .borrow_mut()
                    .add_pending_noallocate_completion(request);
                state.record_sent_coherence_op(forwarded.coherence_op());
                dev_to_mem.put(forwarded)?.await;
                return Ok(());
            }
            let allocate_result =
                allocate_line::<_, AllocateShared>(state, mem_ack, &request, addr).await?;
            match allocate_result {
                AllocateResult::Allocated => {
                    debug!(state.entity ; "{} miss for line 0x{addr:x}; allocate shared and forward", request.id());
                    state.contents.borrow_mut().add_pending_line_waiter(request);
                    state.record_sent_coherence_op(forwarded.coherence_op());
                    dev_to_mem.put(forwarded)?.await;
                }
                AllocateResult::Blocked => {
                    debug!(state.entity ; "{} for line 0x{addr:x} stalled waiting for a free way", request.id());
                    state.contents.borrow_mut().add_pending_line_waiter(request);
                }
                AllocateResult::NeedsWriteback { .. } => unreachable!(),
            }
        }
        Some(_) => unreachable!(),
    }
    Ok(())
}

async fn allocate_line<T, TTransition>(
    state: &RxHandlingState<T>,
    mem_ack: &mut OutPort<T>,
    request: &T,
    addr: u64,
) -> Result<AllocateResult, SimError>
where
    T: SimObject + CoherentAccess,
    TTransition: LineStateTransition,
{
    let allocate_result = { state.contents.borrow_mut().allocate::<TTransition>(addr) };
    let AllocateResult::NeedsWriteback {
        evicted_modified_addr,
    } = allocate_result
    else {
        return Ok(allocate_result);
    };

    state
        .queue_dirty_line_writeback(mem_ack, request, evicted_modified_addr)
        .await?;

    if !state
        .contents
        .borrow_mut()
        .evict_modified(evicted_modified_addr)
    {
        return sim_error!(
            "{}: failed to evict modified line 0x{:x} before allocating access {}",
            state.entity,
            evicted_modified_addr,
            request.id()
        );
    }

    let allocate_result = { state.contents.borrow_mut().allocate::<TTransition>(addr) };
    if matches!(allocate_result, AllocateResult::NeedsWriteback { .. }) {
        return sim_error!(
            "{}: allocation for access {} still requires writeback after evicting line 0x{:x}",
            state.entity,
            request.id(),
            evicted_modified_addr
        );
    }

    Ok(allocate_result)
}

async fn handle_write_request<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    mem_ack: &mut OutPort<T>,
    rsp: &mut OutPort<T>,
    request: T,
    count_metrics: bool,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    let addr = request.dst_addr();
    let line_state = state.contents.borrow().state_for(addr);
    match line_state {
        Some(x) if x.can_write_hit() => {
            if count_metrics {
                state.metrics.record_write_hit();
            }

            debug!(
                state.entity ;
                "{} hit for line 0x{addr:x} in state {}",
                request.id(),
                x.as_str()
            );
            state.complete_local_write(Some(rsp), request).await?;
        }
        Some(x) if x.is_allocated() => {
            if count_metrics {
                state.metrics.record_write_pending_hit();
            }

            debug!(
                state.entity ;
                "{} pending hit for line 0x{addr:x}; queued behind allocation in state {}",
                request.id(),
                x.as_str()
            );
            state.contents.borrow_mut().add_pending_line_waiter(request);
        }
        Some(LineState::Shared) | None => {
            if count_metrics {
                state.metrics.record_write_miss();
            }

            let forwarded = state.prepare_miss_request(&request, CoherenceState::Exclusive)?;
            if !should_allocate(&request) {
                debug!(state.entity ; "{} miss/upgrade for line 0x{addr:x}; forward without allocation", request.id());
                state.contents.borrow_mut().invalidate(addr);
                if request.access_type() == AccessType::WriteNonPostedRequest {
                    state
                        .contents
                        .borrow_mut()
                        .add_pending_noallocate_completion(request);
                } else {
                    state.record_completed_write_payload(&request);
                }
                state.record_sent_coherence_op(forwarded.coherence_op());
                dev_to_mem.put(forwarded)?.await;
                return Ok(());
            }
            let allocate_result =
                allocate_line::<_, AllocateExclusive>(state, mem_ack, &request, addr).await?;
            match allocate_result {
                AllocateResult::Allocated => {
                    debug!(
                        state.entity ;
                        "{} miss/upgrade for line 0x{addr:x}; allocate exclusive and forward",
                        request.id(),
                    );
                    if state.contents.borrow().is_coherent() {
                        state
                            .contents
                            .borrow_mut()
                            .add_pending_exclusive_write(request);
                    } else {
                        let is_nonposted =
                            request.access_type() == AccessType::WriteNonPostedRequest;
                        state.contents.borrow_mut().grant_exclusive(addr, true);
                        if is_nonposted {
                            state
                                .contents
                                .borrow_mut()
                                .add_pending_nonposted_completion(request);
                        } else {
                            state.record_completed_write_payload(&request);
                        }
                    }
                    state.record_sent_coherence_op(forwarded.coherence_op());
                    dev_to_mem.put(forwarded)?.await;
                }
                AllocateResult::Blocked => {
                    debug!(state.entity ; "{} for line 0x{addr:x} stalled waiting for a free way", request.id());
                    state.contents.borrow_mut().add_pending_line_waiter(request);
                }
                AllocateResult::NeedsWriteback { .. } => unreachable!(),
            }
        }
        Some(_) => unreachable!(),
    }
    Ok(())
}

// Memory response path.

async fn run_mem_rx<T>(
    mut state: RxHandlingState<T>,
    mut dev_to_mem: OutPort<T>,
    mut mem_ack: OutPort<T>,
    mut rsp_arb_0: OutPort<T>,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    loop {
        let response = state.rx.get()?.await;
        trace!(state.entity ; "Memory response {}", response);
        let total_bytes = response.total_bytes();
        handle_response(
            &state,
            &mut dev_to_mem,
            &mut mem_ack,
            &mut rsp_arb_0,
            response,
        )
        .await?;
        let ticks = total_bytes.div_ceil(state.bw_bytes_per_cycle);
        state.clock.wait_ticks(ticks as u64).await;
    }
}

async fn handle_response<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    mem_ack: &mut OutPort<T>,
    rsp_arb_0: &mut OutPort<T>,
    access: T,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    state.record_received_coherence_op(access.coherence_op());

    let access_type = access.access_type();
    match access_type {
        AccessType::Control => {
            handle_control_response(state, dev_to_mem, mem_ack, rsp_arb_0, access).await?;
        }
        AccessType::BarrierResponse => {
            complete_forwarded_barrier(state, rsp_arb_0, access).await?;
        }
        AccessType::WriteNonPostedResponse => {
            handle_write_nonposted_response(state, dev_to_mem, rsp_arb_0, access).await?;
        }
        AccessType::ReadRequest
        | AccessType::WriteRequest
        | AccessType::WriteNonPostedRequest
        | AccessType::BarrierRequest => {
            return sim_error!(
                "{}: unsupported {access_type} on response port",
                state.entity
            );
        }
        AccessType::ReadResponse => {
            handle_read_response(state, dev_to_mem, rsp_arb_0, access).await?;
        }
    }

    Ok(())
}

async fn handle_control_response<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    mem_ack: &mut OutPort<T>,
    rsp_arb_0: &mut OutPort<T>,
    access: T,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    match access.coherence_op() {
        Some(CoherenceOp::Invalidate) => {
            handle_invalidate(state, dev_to_mem, mem_ack, rsp_arb_0, &access).await?;
        }
        Some(CoherenceOp::GrantExclusive) => {
            let completed_noallocate = state.complete_noallocate_access(rsp_arb_0, &access).await?;
            if !completed_noallocate {
                state
                    .apply_grant_exclusive(Some(rsp_arb_0), &access)
                    .await?;
            }
            process_pending_line_waiters_for_line(state, dev_to_mem, rsp_arb_0, &access).await?;
        }
        _ => {
            return sim_error!(
                "{}: unsupported coherence op {:?} on memory control access {} for line 0x{:x}",
                state.entity,
                access.coherence_op(),
                access.id(),
                access.dst_addr()
            );
        }
    }
    Ok(())
}

async fn handle_write_nonposted_response<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    rsp_arb_0: &mut OutPort<T>,
    access: T,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    let response_line = access.clone();
    let completed_noallocate = state.complete_noallocate_access(rsp_arb_0, &access).await?;
    if !completed_noallocate {
        if access.coherence_op() == Some(CoherenceOp::GrantExclusive) {
            state
                .apply_grant_exclusive(Some(rsp_arb_0), &access)
                .await?;
        } else if !state.contents.borrow().is_coherent() {
            state
                .complete_pending_non_posted_writes(rsp_arb_0, &access)
                .await?;
        } else {
            debug!(
                state.entity ;
                "Write response access {} without grant for line 0x{:x}; invalidate and forward",
                access.id(),
                access.dst_addr()
            );
            state.contents.borrow_mut().invalidate(access.dst_addr());
            rsp_arb_0.put(access)?.await;
        }
    }
    process_pending_line_waiters_for_line(state, dev_to_mem, rsp_arb_0, &response_line).await
}

async fn handle_read_response<T>(
    state: &RxHandlingState<T>,
    dev_to_mem: &mut OutPort<T>,
    rsp_arb_0: &mut OutPort<T>,
    access: T,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    let completed_noallocate = state.complete_noallocate_access(rsp_arb_0, &access).await?;
    if !completed_noallocate {
        if access.coherence_op() == Some(CoherenceOp::GrantExclusive) {
            state.apply_grant_exclusive(None, &access).await?;
        } else {
            debug!(
                state.entity ;
                "Read response access {} for line 0x{:x}; set state {}",
                access.id(),
                access.dst_addr(),
                LineState::Shared.as_str()
            );
            state
                .contents
                .borrow_mut()
                .transition::<GrantShared>(access.dst_addr());
        }
    }
    process_pending_line_waiters_for_line(state, dev_to_mem, rsp_arb_0, &access).await
}

// Shared request/response helpers.

fn log_cache_forward<T>(entity: &Rc<Entity>, label: &str, original: &T, forwarded: &T)
where
    T: SimObject + CoherentAccess,
{
    debug!(
        entity ;
        "{} access {} for addr 0x{:x}: {} -> {} via {} -> {} coherence {:?}",
        label,
        original.id(),
        original.dst_addr(),
        original.access_type(),
        forwarded.access_type(),
        original.src_device(),
        forwarded.dst_device(),
        forwarded.coherence_op()
    );
}

fn should_allocate<T>(request: &T) -> bool
where
    T: CoherentAccess,
{
    request.cache_hint() == CacheHintType::Allocate
}

#[cfg(test)]
mod test_helpers {
    use std::rc::Rc;

    use gwr_engine::types::AccessType;
    use gwr_track::entity::Entity;

    use super::CacheConfig;
    use crate::memory::memory_access::MemoryAccess;
    use crate::memory::memory_map::{DeviceId, MemoryMap};

    pub(super) fn test_config(num_sets: usize, num_ways: usize) -> CacheConfig {
        let memory_map = Rc::new(MemoryMap::from_regions(&[(0, u64::MAX, DeviceId(0))]).unwrap());
        CacheConfig::new(DeviceId(0), 32, 32, num_sets, num_ways, 8, &memory_map)
    }

    pub(super) fn test_access(
        entity: &Rc<Entity>,
        access_type: AccessType,
        addr: u64,
        src_device: DeviceId,
    ) -> MemoryAccess {
        MemoryAccess::new(
            entity,
            access_type,
            32,
            addr,
            0x1000,
            DeviceId(9),
            src_device,
            8,
        )
    }
}
