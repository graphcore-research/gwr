// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! A range-correct whole-line n-way cache with round-robin replacement.
//!
//! Ordinary operation provides no memory ordering guarantees. Optional
//! coherence uses directory-managed shared/exclusive permissions and dirty
//! writeback through the same bounded parent/child tracking. Coherent barriers
//! drain prior work; they do not flush resident dirty lines. Coherent hierarchy
//! is not modelled.
//!
//! Reads round missing data to whole cache lines. Writes invalidate every cache
//! line they touch and are forwarded downstream at their original address and
//! size. In-flight fills have independent owner identities that protect against
//! stale responses. No numerical data content, vendor set hashing or hardware
//! policy is implied.
//!
//! Transactions passing through this cache must implement
//! [`CacheAccess`], which extends
//! [`AccessMemory`](crate::memory::traits::AccessMemory) with allocation
//! policy, fill creation, and fill identity. Ordinary memories require only
//! `AccessMemory`.
//!
//! # Concepts and terminology
//!
//! - **Parent:** an original read request received from the device side. The
//!   cache tracks its dependencies and returns one response for the original
//!   range once all required fills have completed. For example, a read spanning
//!   three missing lines is one parent waiting for three fills. In nested
//!   caches, a fill from an upper cache becomes a parent in the lower cache.
//! - **Fill (child request):** an internal downstream read of one aligned,
//!   whole cache line. Its response serves the parents waiting for that line.
//! - **Coalescing:** an allocating read joins an existing pending fill instead
//!   of issuing another read for the same line. Several parents can therefore
//!   share one fill. Waiting for a fill counts as a request miss; a request hit
//!   requires every needed line to be resident and valid already.
//! - **Line, set, and way:** a line is the unit of fetching and validity. Its
//!   address selects a set, whose ways are the available storage slots; a tag
//!   identifies the line occupying a way. Replacement selects ways in
//!   round-robin order within each set.
//! - **Validity and fill ownership:** a line is missing, pending a particular
//!   fill, or valid. Each fill has a cache-owned identity token, preserved in
//!   its response. Replacement or invalidation prevents an older fill from
//!   installing data into a slot that no longer belongs to it.
//! - **Allocation:** allocating reads populate the cache on a miss.
//!   `NoAllocate` reads can hit valid lines, but their misses fetch whole lines
//!   without installing them. Writes invalidate every cache line they touch and
//!   are forwarded downstream at their original address and size.
//! - **Admission limits:** `max_inflight_parents` bounds tracked original
//!   reads; `max_inflight_fills` bounds downstream line reads awaiting
//!   responses. Large requests issue fills incrementally as slots become
//!   available, so a parent can require more fills than fit concurrently.
//! - **Service:** bandwidth accounting for packet payload and headers. A fill
//!   becomes available only after its full response ingress service; hits and
//!   completed misses share the client return-service bandwidth. All outgoing
//!   memory-side requests, writebacks, and acknowledgements share one
//!   [`Limiter`] after arbitration and request latency. Downstream backpressure
//!   overlaps its packet service interval rather than adding another interval.
//! - **Control ticket:** a cache-local sequence number assigned when a
//!   writeback or invalidation acknowledgement enters the shared FIFO control
//!   queue. The submission count supplies the ticket; the completion count
//!   advances in the same order after downstream acceptance and output service
//!   finish. A producer waits until the completion count reaches its ticket
//!   before continuing. This preserves writeback-before-acknowledgement and
//!   writeback-before-acquisition ordering across workers. Tickets stay inside
//!   the cache; they are distinct from fill identities and invalidation tokens
//!   carried by packets.
//!
//! ```text
//!  ----------------------------
//!  |          Device          |
//!  ----------------------------
//!       |               |
//!       |               |
//!  ----------------------------
//!  |  dev_rx          dev_tx  |
//!  |    |               ^     |
//!  |    |               |     |
//!  |    |             delay   |
//!  |    |               |     |
//!  |    |        0  response  |
//!  |    +---------> arbiter   |
//!  |    |               ^     |
//!  |  req arbiter       | 1   |
//!  |    |               |     |
//!  |  delay             |     |
//!  |    |               |     |
//!  |  limiter           |     |
//!  |    |     Cache     |     |
//!  |    v               |     |
//!  |  mem_tx          mem_rx  |
//!  ----------------------------
//!       |              |
//!       |              |
//!  ----------------------------
//!  |         Mem/Bus          |
//!  ----------------------------
//! ```
pub mod coherency_manager;
pub mod config;
pub mod traits;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt::{self, Display};
use std::rc::Rc;

use async_trait::async_trait;
pub use config::CacheConfig;
use gwr_components::arbiter::Arbiter;
use gwr_components::arbiter::policy::{Priority, PriorityRoundRobin, RoundRobin};
use gwr_components::delay::Delay;
use gwr_components::flow_controls::limiter::Limiter;
use gwr_components::flow_controls::rate_limiter::RateLimiter;
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
use gwr_track::{build_aka, trace};

use crate::log_stats;
use crate::memory::traits::{CacheAccess, ReadMemory};

#[derive(Clone, Default)]
struct CacheMetrics {
    coherence_sent: [usize; 7],
    coherence_received: [usize; 7],
    payload_bytes_read: usize,
    payload_bytes_written: usize,
    num_hits: usize,
    num_misses: usize,
    line_hits: usize,
    line_misses: usize,
    coalesced_lines: usize,
    fill_requests: usize,
    downstream_read_bytes: usize,
    downstream_write_bytes: usize,
    downstream_request_header_bytes: usize,
    downstream_response_header_bytes: usize,
}

/// Completed native cache activity; payload and transport overhead are
/// separate.
#[derive(Clone, Debug, Default)]
pub struct CacheTraffic {
    /// Sent protocol operations, indexed by CoherenceOp in declaration order.
    pub coherence_sent: [usize; 7],
    /// Received protocol operations, indexed by CoherenceOp in declaration
    /// order.
    pub coherence_received: [usize; 7],
    /// Original logical client read payload.
    pub logical_read_bytes: usize,
    /// Original logical client write payload.
    pub logical_write_bytes: usize,
    /// Requests where every cache line was already resident.
    pub request_hits: usize,
    /// Requests requiring a new or coalesced pending fill.
    pub request_misses: usize,
    /// Requested lines initially containing valid data.
    pub line_hits: usize,
    /// Lines issuing new full-line fills.
    pub line_misses: usize,
    /// Lines joining an already pending allocating fill.
    pub coalesced_lines: usize,
    /// Distinct internal fill transactions.
    pub fill_requests: usize,
    /// Actual rounded downstream read payload.
    pub downstream_read_bytes: usize,
    /// Actual forwarded original write payload.
    pub downstream_write_bytes: usize,
    /// Headers sent downstream, excluding read/write payload.
    pub downstream_request_header_bytes: usize,
    /// Headers received downstream, excluding response payload.
    pub downstream_response_header_bytes: usize,
}

/// Current and peak fill counts governed by `max_inflight_fills`.
/// Counts include invalidated fills still awaiting responses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheInflightFills {
    /// Current fills awaiting responses.
    pub current: usize,
    /// Largest number of fills awaiting responses at the same time.
    pub peak: usize,
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

mod contents;
use contents::{CacheContents, LineAction};

pub mod coherence;
pub use coherence::CoherenceConfig;

use crate::cache::coherency_manager::CoherenceOp;
use crate::cache::traits::CoherentAccess;

impl<T> ReadMemory for CacheContents<T>
where
    T: SimObject + CacheAccess,
{
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

impl<T> ReadMemory for RefCell<CacheContents<T>>
where
    T: SimObject + CacheAccess,
{
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

#[derive(EntityGet, EntityDisplay)]
pub struct Cache<T>
where
    T: SimObject + CacheAccess,
{
    entity: Rc<Entity>,

    clock: Clock,
    spawner: Spawner,
    metrics: Rc<RefCell<CacheMetrics>>,
    contents: Rc<RefCell<CacheContents<T>>>,
    state_changed: Repeated<()>,

    response_delay: RefCell<Option<Rc<Delay<T>>>>,
    memory_tx_rx: RefCell<Option<InPort<T>>>,
    memory_output_limiter: Rc<Limiter<T>>,
    mem_tx: RefCell<Option<OutPort<T>>>,

    dev_rx: RefCell<Option<InPort<T>>>,
    mem_rx: RefCell<Option<InPort<T>>>,

    bandwidth_bytes_per_tick: usize,

    // Internal ports
    req: RefCell<Option<OutPort<T>>>,
    control: RefCell<Option<OutPort<T>>>,
    rsp_arb_0: RefCell<Option<OutPort<T>>>,
    rsp_arb_1: RefCell<Option<OutPort<T>>>,
}

impl<T: SimObject + CoherentAccess> Cache<T> {
    pub fn new_coherent_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        config: CacheConfig,
        coherence: CoherenceConfig,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_coherent_and_register_with_renames(
            engine, clock, parent, name, None, config, coherence,
        )
    }

    pub fn new_coherent_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        config: CacheConfig,
        coherence: CoherenceConfig,
    ) -> Result<Rc<Self>, SimError> {
        coherence.validate(config.line_size_bytes())?;
        let cache = Self::new_and_register_with_renames(engine, clock, parent, name, aka, config)?;
        cache.contents.borrow_mut().coherence =
            Some(Rc::new(coherence::DirectoryCoherenceProtocol(coherence)));
        Ok(cache)
    }
}

impl<T> Cache<T>
where
    T: SimObject + CacheAccess,
{
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

    /// Create an instance of the cache and register it with the Engine.
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        config: CacheConfig,
    ) -> Result<Rc<Self>, SimError> {
        config
            .validate()
            .map_err(|error| SimError(format!("{parent}::{name}: {error}")))?;
        let bandwidth_bytes_per_tick = config.bw_bytes_per_tick;
        let entity = Rc::new(Entity::new(parent, name));

        let policy = Box::new(RoundRobin::new());
        let response_arbiter =
            Arbiter::new_and_register(engine, clock, &entity, "rsp_arb", 2, policy);

        let response_delay_aka = build_aka!(aka, &entity, &[("dev_tx", "tx")]);
        let response_delay = Delay::new_and_register_with_renames(
            engine,
            clock,
            &entity,
            "rsp_delay",
            Some(&response_delay_aka),
            config.delay_ticks,
        );

        response_arbiter
            .connect_port_tx(response_delay.port_rx())
            .expect("Internal ports should connect without error");

        // Create internal ports that are driven by the cache logic
        let mut req = OutPort::new(&entity, "req_arb_0");
        let request_arbiter = Arbiter::new_and_register(
            engine,
            clock,
            &entity,
            "req_arb",
            2,
            Box::new(PriorityRoundRobin::<Priority>::new(2).set_priority(1, Priority::High)),
        );
        let memory_tx_rx = InPort::new(engine, clock, &entity, "memory_tx_rx");
        let limiter_aka = build_aka!(aka, &entity, &[("mem_tx", "tx")]);
        let bits_per_tick = bandwidth_bytes_per_tick.checked_mul(8).ok_or_else(|| {
            SimError(format!(
                "{entity}: memory output bandwidth in bits overflows"
            ))
        })?;
        let memory_output_limiter = Limiter::new_and_register_with_renames(
            engine,
            clock,
            &entity,
            "memory_output_limiter",
            Some(&limiter_aka),
            Rc::new(RateLimiter::new(clock, bits_per_tick)?),
        );
        let mut mem_tx = OutPort::new(&entity, "memory_output");
        mem_tx.connect(memory_output_limiter.port_rx())?;
        if config.delay_ticks == 0 {
            request_arbiter.connect_port_tx(memory_tx_rx.state())?;
        } else {
            let request_delay =
                Delay::new_and_register(engine, clock, &entity, "req_delay", config.delay_ticks);
            request_arbiter.connect_port_tx(request_delay.port_rx())?;
            request_delay.connect_port_tx(memory_tx_rx.state())?;
        }
        let mut control = OutPort::new(&entity, "control");
        control.connect(request_arbiter.port_rx_i(1))?;
        req.connect(request_arbiter.port_rx_i(0))
            .expect("Internal ports should connect without error");

        let mut rsp_arb_0 = OutPort::new(&entity, "rsp_arb_0");
        rsp_arb_0
            .connect(response_arbiter.port_rx_i(0))
            .expect("Internal ports should connect without error");

        let mut rsp_arb_1 = OutPort::new(&entity, "rsp_arb_1");
        rsp_arb_1
            .connect(response_arbiter.port_rx_i(1))
            .expect("Internal ports should connect without error");

        let dev_rx = InPort::new_with_renames(engine, clock, &entity, "dev_rx", aka);
        let mem_rx = InPort::new_with_renames(engine, clock, &entity, "mem_rx", aka);

        let spawner = engine.spawner();
        let rc_self = Rc::new(Self {
            entity: entity.clone(),
            clock: clock.clone(),
            spawner,
            metrics: Rc::new(RefCell::new(CacheMetrics::default())),
            contents: Rc::new(RefCell::new(CacheContents::new(&entity, config))),
            state_changed: Repeated::default(),
            response_delay: RefCell::new(Some(response_delay)),
            memory_tx_rx: RefCell::new(Some(memory_tx_rx)),
            memory_output_limiter,
            mem_tx: RefCell::new(Some(mem_tx)),
            dev_rx: RefCell::new(Some(dev_rx)),
            mem_rx: RefCell::new(Some(mem_rx)),
            bandwidth_bytes_per_tick,

            req: RefCell::new(Some(req)),
            control: RefCell::new(Some(control)),
            rsp_arb_0: RefCell::new(Some(rsp_arb_0)),
            rsp_arb_1: RefCell::new(Some(rsp_arb_1)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn connect_port_dev_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        connect_tx!(self.response_delay, connect_port_tx ; port_state)
    }

    pub fn connect_port_mem_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        self.memory_output_limiter.connect_port_tx(port_state)
    }

    pub fn port_dev_rx(&self) -> PortStateResult<T> {
        port_rx!(self.dev_rx, state)
    }

    pub fn port_mem_rx(&self) -> PortStateResult<T> {
        port_rx!(self.mem_rx, state)
    }

    #[must_use]
    pub fn payload_bytes_read(&self) -> usize {
        self.metrics.borrow().payload_bytes_read
    }

    #[must_use]
    pub fn payload_bytes_written(&self) -> usize {
        self.metrics.borrow().payload_bytes_written
    }

    #[must_use]
    pub fn num_hits(&self) -> usize {
        self.metrics.borrow().num_hits
    }

    #[must_use]
    pub fn num_misses(&self) -> usize {
        self.metrics.borrow().num_misses
    }

    /// Snapshot actual activity. Pending requests are not completion claims.
    pub fn traffic_snapshot(&self) -> CacheTraffic {
        let metrics = self.metrics.borrow();
        CacheTraffic {
            coherence_sent: metrics.coherence_sent,
            coherence_received: metrics.coherence_received,
            logical_read_bytes: metrics.payload_bytes_read,
            logical_write_bytes: metrics.payload_bytes_written,
            request_hits: metrics.num_hits,
            request_misses: metrics.num_misses,
            line_hits: metrics.line_hits,
            line_misses: metrics.line_misses,
            coalesced_lines: metrics.coalesced_lines,
            fill_requests: metrics.fill_requests,
            downstream_read_bytes: metrics.downstream_read_bytes,
            downstream_write_bytes: metrics.downstream_write_bytes,
            downstream_request_header_bytes: metrics.downstream_request_header_bytes,
            downstream_response_header_bytes: metrics.downstream_response_header_bytes,
        }
    }

    /// Snapshot current and peak usage of the `max_inflight_fills` limit.
    #[must_use]
    pub fn inflight_fills_snapshot(&self) -> CacheInflightFills {
        self.contents.borrow().inflight_fills_snapshot()
    }

    pub fn dump_stats(&self, time_now_ns: f64) {
        let metrics = self.metrics.borrow();
        log_stats(
            &self.entity,
            CacheStatsDisplay::new(
                format!("Cache {}", self.entity.full_name()),
                time_now_ns,
                metrics.payload_bytes_read,
                metrics.payload_bytes_written,
                metrics.num_hits,
                metrics.num_misses,
            ),
        );
    }
}

#[derive(Clone)]
struct CacheState<T>
where
    T: SimObject + CacheAccess,
{
    entity: Rc<Entity>,
    clock: Clock,
    contents: Rc<RefCell<CacheContents<T>>>,
    metrics: Rc<RefCell<CacheMetrics>>,
    state_changed: Repeated<()>,
    bandwidth_bytes_per_tick: usize,
    /// Completed coherent parents awaiting client response service.
    completed_parent_requests: Rc<RefCell<VecDeque<T>>>,
    /// Writebacks and invalidation acknowledgements awaiting memory output.
    pending_control_packets: Rc<RefCell<VecDeque<T>>>,
    /// Cumulative submissions; each value is also a producer completion ticket.
    submitted_control_count: Rc<Cell<u64>>,
    /// Cumulative control packets that have completed memory output service.
    completed_control_count: Rc<Cell<u64>>,
}

#[async_trait(?Send)]
impl<T> Runnable for Cache<T>
where
    T: SimObject + CacheAccess,
{
    async fn run(&self) -> SimResult {
        let state = CacheState {
            entity: self.entity.clone(),
            clock: self.clock.clone(),
            contents: self.contents.clone(),
            metrics: self.metrics.clone(),
            state_changed: self.state_changed.clone(),
            bandwidth_bytes_per_tick: self.bandwidth_bytes_per_tick,
            completed_parent_requests: Rc::new(RefCell::new(VecDeque::new())),
            pending_control_packets: Rc::new(RefCell::new(VecDeque::new())),
            submitted_control_count: Rc::new(Cell::new(0)),
            completed_control_count: Rc::new(Cell::new(0)),
        };
        let memory_tx_rx = take_option!(self.memory_tx_rx);
        let mem_tx = take_option!(self.mem_tx);
        let memory_tx_state = state.clone();
        self.spawner.spawn(async move {
            forward_memory_requests(memory_tx_rx, memory_tx_state, mem_tx).await
        });
        let dev_state = state.clone();
        let dev_rx = take_option!(self.dev_rx);
        let req = take_option!(self.req);
        let control = take_option!(self.control);
        let control_state = state.clone();
        self.spawner
            .spawn(async move { run_control_tx(control_state, control).await });
        let rsp_arb_1 = take_option!(self.rsp_arb_1);
        self.spawner
            .spawn(async move { run_dev_rx(dev_rx, dev_state, req, rsp_arb_1).await });
        let mut rsp_arb_0 = Some(take_option!(self.rsp_arb_0));
        if self.contents.borrow().coherence.is_some() {
            let mut tx = rsp_arb_0.take().unwrap();
            let return_state = state.clone();
            self.spawner.spawn(async move {
                loop {
                    let parent_request = return_state
                        .completed_parent_requests
                        .borrow_mut()
                        .pop_front();
                    if let Some(parent_request) = parent_request {
                        complete_parent_request(&return_state, &mut tx, parent_request).await?;
                    } else {
                        return_state.state_changed.listen().await;
                    }
                }
            });
        }
        let mem_rx = take_option!(self.mem_rx);
        run_mem_rx(mem_rx, state, rsp_arb_0).await
    }
}

async fn run_dev_rx<T>(
    mut rx: InPort<T>,
    state: CacheState<T>,
    mut req: OutPort<T>,
    mut rsp_arb_1: OutPort<T>,
) -> SimResult
where
    T: SimObject + CacheAccess,
{
    loop {
        let request = rx.get()?.await;
        trace!(state.entity ; "Device request {}", request);
        let total_bytes = request.total_bytes();
        let ticks = total_bytes.div_ceil(state.bandwidth_bytes_per_tick);
        state.clock.wait_ticks(ticks as u64)?.await;
        handle_device_request(&state, &mut req, &mut rsp_arb_1, request).await?;
    }
}

async fn run_mem_rx<T>(
    mut rx: InPort<T>,
    state: CacheState<T>,
    mut rsp: Option<OutPort<T>>,
) -> SimResult
where
    T: SimObject + CacheAccess,
{
    loop {
        let response = rx.get()?.await;
        trace!(state.entity ; "Memory response {}", response);
        let total_bytes = response.total_bytes();
        let ticks = total_bytes.div_ceil(state.bandwidth_bytes_per_tick);
        state.clock.wait_ticks(ticks as u64)?.await;
        if let Some(op) = state
            .contents
            .borrow()
            .coherence
            .as_ref()
            .and_then(|p| p.coherence_op(&response))
        {
            state.metrics.borrow_mut().coherence_received[op as usize] += 1;
        }
        let invalidation = state
            .contents
            .borrow()
            .coherence
            .as_ref()
            .is_some_and(|p| p.coherence_op(&response) == Some(CoherenceOp::Invalidate));
        if invalidation {
            let messages = state
                .contents
                .borrow_mut()
                .handle_coherence_invalidation(&response)?;
            state.state_changed.notify();
            for packet in messages {
                send_control(&state, packet).await?;
            }
        } else {
            handle_memory_response(&state, &mut rsp, response).await?;
        }
    }
}

async fn handle_device_request<T>(
    state: &CacheState<T>,
    req: &mut OutPort<T>,
    rsp: &mut OutPort<T>,
    request: T,
) -> SimResult
where
    T: SimObject + CacheAccess,
{
    let address = request.dst_addr();
    match request.access_type() {
        AccessType::Control => {
            if state.contents.borrow().coherence.is_some() {
                let packets = state
                    .contents
                    .borrow_mut()
                    .handle_coherence_invalidation(&request)?;
                for packet in packets {
                    send_control(state, packet).await?;
                }
            } else {
                state
                    .contents
                    .borrow_mut()
                    .invalidate_range(address, request.access_size_bytes())?;
            }
            state.state_changed.notify();
        }
        AccessType::ReadRequest => handle_parent_request(state, req, rsp, request).await?,
        AccessType::WriteRequest | AccessType::WriteNonPostedRequest => {
            if state.contents.borrow().coherence.is_some() {
                return handle_parent_request(state, req, rsp, request).await;
            }
            state
                .contents
                .borrow()
                .checked_range_end_exclusive(address, request.access_size_bytes())?;
            state
                .contents
                .borrow_mut()
                .invalidate_range(address, request.access_size_bytes())?;
            let bytes = request.access_size_bytes();
            {
                let mut metrics = state.metrics.borrow_mut();
                metrics.payload_bytes_written += bytes;
                metrics.downstream_write_bytes += bytes;
                metrics.downstream_request_header_bytes += request
                    .total_bytes()
                    .checked_sub(bytes)
                    .ok_or_else(|| SimError("write packet shorter than payload".into()))?;
            }
            req.put(request)?.await;
        }
        AccessType::BarrierRequest => {
            while !state.contents.borrow().is_idle() {
                state.state_changed.listen().await;
            }
            let forwarded = state.contents.borrow_mut().begin_barrier(&request)?;
            if let Some(forwarded) = forwarded {
                req.put(forwarded)?.await;
                loop {
                    if state
                        .contents
                        .borrow_mut()
                        .barrier_response
                        .take()
                        .is_some()
                    {
                        break;
                    }
                    state.state_changed.listen().await;
                }
            }
            complete_parent_request(state, rsp, request).await?;
        }
        other => {
            return sim_error!(
                "{}: unsupported AccessType from device: {other}",
                state.entity
            );
        }
    }
    Ok(())
}

async fn run_control_tx<T: SimObject + CacheAccess>(
    state: CacheState<T>,
    mut tx: OutPort<T>,
) -> SimResult {
    loop {
        let next = state.pending_control_packets.borrow_mut().pop_front();
        if let Some(packet) = next {
            tx.put(packet)?.await;
        } else {
            state.state_changed.listen().await;
        }
    }
}

/// Forward selected packets through the output limiter. Its upstream handoff
/// completes after downstream acceptance and service, so control completion
/// preserves writeback ordering without duplicating bandwidth accounting.
async fn forward_memory_requests<T: SimObject + CacheAccess>(
    mut rx: InPort<T>,
    state: CacheState<T>,
    mut tx: OutPort<T>,
) -> SimResult {
    loop {
        let packet = rx.start_get()?.await;
        // Only these operations use the FIFO control queue. Its completion
        // count follows the same order, without attaching transport metadata.
        let completes_control_ticket =
            state
                .contents
                .borrow()
                .coherence
                .as_ref()
                .is_some_and(|protocol| {
                    matches!(
                        protocol.coherence_op(&packet),
                        Some(CoherenceOp::Writeback | CoherenceOp::InvalidateAck)
                    )
                });
        tx.put(packet)?.await;
        if completes_control_ticket {
            state
                .completed_control_count
                .set(state.completed_control_count.get() + 1);
            state.state_changed.notify();
        }
        rx.finish_get();
    }
}

async fn handle_parent_request<T: SimObject + CacheAccess>(
    state: &CacheState<T>,
    req: &mut OutPort<T>,
    rsp: &mut OutPort<T>,
    request: T,
) -> SimResult {
    let address = request.dst_addr();

    // Validate the whole original range before any admission or
    // metrics.
    state
        .contents
        .borrow()
        .checked_range_end_exclusive(address, request.access_size_bytes())?;
    loop {
        if state.contents.borrow().has_parent_capacity() {
            break;
        }
        state.state_changed.listen().await;
    }
    if request.access_type() == AccessType::ReadRequest {
        state.metrics.borrow_mut().payload_bytes_read += request.access_size_bytes();
    } else {
        state.metrics.borrow_mut().payload_bytes_written += request.access_size_bytes();
    }
    let (parent_id, mut current_line_address, end) =
        state.contents.borrow_mut().admit_request(request)?;
    while current_line_address < end {
        let action = state.contents.borrow_mut().process_parent_line(
            parent_id,
            current_line_address,
            end,
            &mut state.metrics.borrow_mut(),
        )?;
        match action {
            LineAction::Advance(next) => current_line_address = next,
            LineAction::Send(line_read_request, next) => {
                if let Some(op) = state
                    .contents
                    .borrow()
                    .coherence
                    .as_ref()
                    .and_then(|p| p.coherence_op(&line_read_request))
                {
                    state.metrics.borrow_mut().coherence_sent[op as usize] += 1;
                }
                req.put(line_read_request)?.await;
                current_line_address = next;
            }
            LineAction::Writeback(packet) => send_control(state, packet).await?,
            LineAction::Blocked => state.state_changed.listen().await,
        }
    }
    let ready = state
        .contents
        .borrow_mut()
        .finish_issuing_parent(parent_id, &mut state.metrics.borrow_mut());
    if let Some(parent_request) = ready {
        state.state_changed.notify();
        complete_parent_request(state, rsp, parent_request).await?;
    }
    Ok(())
}

async fn handle_memory_response<T: SimObject + CacheAccess>(
    state: &CacheState<T>,
    rsp: &mut Option<OutPort<T>>,
    response: T,
) -> SimResult {
    let coherent = state.contents.borrow().coherence.is_some();
    match response.access_type() {
        AccessType::BarrierResponse => {
            state.contents.borrow_mut().complete_barrier(response)?;
            state.state_changed.notify();
            return Ok(());
        }
        AccessType::Control if !coherent => return Ok(()),
        AccessType::WriteNonPostedResponse if !coherent => {
            state.metrics.borrow_mut().downstream_response_header_bytes += response.total_bytes();
            return send_client_response(state, rsp.as_mut().unwrap(), response).await;
        }
        AccessType::Control | AccessType::WriteNonPostedResponse | AccessType::ReadResponse => {}
        other => return sim_error!("{}: unsupported {other} on response port", state.entity),
    }
    let payload = if response.access_type() == AccessType::ReadResponse {
        response.access_size_bytes()
    } else {
        0
    };
    let header = response
        .total_bytes()
        .checked_sub(payload)
        .ok_or_else(|| SimError("response shorter than payload".into()))?;
    let ready = state
        .contents
        .borrow_mut()
        .complete_child_request(&response)?;
    let packets = state
        .contents
        .borrow_mut()
        .complete_deferred_invalidations()?;
    for packet in packets {
        send_control(state, packet).await?;
    }
    state.metrics.borrow_mut().downstream_response_header_bytes += header;
    if coherent {
        state.completed_parent_requests.borrow_mut().extend(ready);
    } else {
        for parent_request in ready {
            complete_parent_request(state, rsp.as_mut().unwrap(), parent_request).await?;
        }
    }
    state.state_changed.notify();
    Ok(())
}

/// Queue a writeback or invalidation acknowledgement. A single FIFO preserves
/// writeback-before-ack across both ingress workers.
/// Each producer waits for shared output service, so at most two messages
/// queue. This also keeps a writeback ahead of any subsequent permission
/// acquisition.
async fn send_control<T: SimObject + CacheAccess>(state: &CacheState<T>, packet: T) -> SimResult {
    if let Some(op) = state
        .contents
        .borrow()
        .coherence
        .as_ref()
        .and_then(|p| p.coherence_op(&packet))
    {
        state.metrics.borrow_mut().coherence_sent[op as usize] += 1;
    }
    let payload = if packet.access_type() == AccessType::WriteRequest {
        packet.access_size_bytes()
    } else {
        0
    };
    state.metrics.borrow_mut().downstream_write_bytes += payload;
    state.metrics.borrow_mut().downstream_request_header_bytes += packet.total_bytes() - payload;
    let ticket = state
        .submitted_control_count
        .get()
        .checked_add(1)
        .ok_or_else(|| SimError("control identity exhausted".into()))?;
    state.submitted_control_count.set(ticket);
    state.pending_control_packets.borrow_mut().push_back(packet);
    state.state_changed.notify();
    while state.completed_control_count.get() < ticket {
        state.state_changed.listen().await;
    }
    Ok(())
}

async fn complete_parent_request<T>(
    state: &CacheState<T>,
    tx: &mut OutPort<T>,
    request: T,
) -> SimResult
where
    T: SimObject + CacheAccess,
{
    let counted = request.access_type() != AccessType::BarrierRequest;
    if request.access_type() != AccessType::WriteRequest {
        let response = if request.access_type() == AccessType::ReadRequest {
            request.to_response(state.contents.as_ref())?
        } else {
            request.to_ack()?
        };
        send_client_response(state, tx, response).await?;
    }
    if counted {
        state.contents.borrow_mut().complete_parent_return();
        state.state_changed.notify();
    }
    Ok(())
}

async fn send_client_response<T>(
    state: &CacheState<T>,
    tx: &mut OutPort<T>,
    response: T,
) -> SimResult
where
    T: SimObject + CacheAccess,
{
    // One shared client-return service timeline, including resident hits.
    // Incoming memory responses and request headers retain their own port
    // rates.
    let now = state.clock.tick_now().tick();
    let end = {
        let mut contents = state.contents.borrow_mut();
        let start = now.max(contents.response_service_end);
        let ticks = u64::try_from(
            response
                .total_bytes()
                .div_ceil(state.bandwidth_bytes_per_tick),
        )
        .map_err(|e| SimError(e.to_string()))?;
        let end = start
            .checked_add(ticks)
            .ok_or_else(|| SimError("cache return service timeline overflows".into()))?;
        contents.response_service_end = end;
        end
    };
    state.clock.wait_ticks(end - now)?.await;
    tx.put(response)?.await;
    Ok(())
}

#[cfg(test)]
mod tests;
