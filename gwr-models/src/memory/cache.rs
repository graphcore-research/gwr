// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! A range-correct whole-line n-way cache with round-robin replacement.
//!
//! The cache provides no memory ordering guarantees.
//!
//! Reads round missing data to whole cache lines. Writes invalidate every cache
//! line they touch and are forwarded downstream at their original address and
//! size. In-flight fills
//! have independent owner identities that protect against stale responses.
//! No numerical data content, vendor set hashing or hardware policy is implied.
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
//!   completed misses share the client return-service bandwidth.
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
//!  |  delay             | 1   |
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
use std::cell::RefCell;
use std::fmt::{self, Display};
use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::arbiter::Arbiter;
use gwr_components::arbiter::policy::RoundRobin;
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
use gwr_track::{build_aka, trace};

use crate::log_stats;
use crate::memory::traits::{CacheAccess, ReadMemory};

#[derive(Clone)]
pub struct CacheConfig {
    line_size_bytes: usize,
    max_inflight_parents: usize,
    max_inflight_fills: usize,
    bw_bytes_per_tick: usize,
    num_sets: usize,
    num_ways: usize,
    delay_ticks: usize,
}

impl CacheConfig {
    #[must_use]
    pub fn new(
        line_size_bytes: usize,
        bw_bytes_per_tick: usize,
        num_sets: usize,
        num_ways: usize,
        delay_ticks: usize,
    ) -> Self {
        Self {
            line_size_bytes,
            max_inflight_parents: 64,
            max_inflight_fills: 64,
            bw_bytes_per_tick,
            num_sets,
            num_ways,
            delay_ticks,
        }
    }

    /// Bytes fetched by each downstream read miss.
    #[must_use]
    pub fn line_size_bytes(&self) -> usize {
        self.line_size_bytes
    }

    /// Set separate limits on tracked original reads (parents) and downstream
    /// line reads awaiting responses (fills). A parent remains tracked until
    /// all its required lines have been processed and its pending fills
    /// complete. Larger valid requests stream through these bounds.
    #[must_use]
    pub fn with_admission_limits(
        mut self,
        max_inflight_parents: usize,
        max_inflight_fills: usize,
    ) -> Self {
        self.max_inflight_parents = max_inflight_parents;
        self.max_inflight_fills = max_inflight_fills;
        self
    }

    pub fn validate(&self) -> SimResult {
        if self.line_size_bytes == 0 {
            return sim_error!("cache line size must be greater than zero");
        }
        if self.max_inflight_parents == 0 || self.max_inflight_fills == 0 {
            return sim_error!("cache admission bounds must be positive");
        }
        if self.bw_bytes_per_tick == 0 {
            return sim_error!("cache bandwidth must be greater than zero");
        }
        if self.num_sets == 0 {
            return sim_error!("cache set count must be greater than zero");
        }
        if self.num_ways == 0 {
            return sim_error!("cache way count must be greater than zero");
        }
        self.num_sets
            .checked_mul(self.num_ways)
            .and_then(|n| n.checked_mul(self.line_size_bytes))
            .ok_or_else(|| SimError("cache entry count overflows".to_string()))?;
        Ok(())
    }
}

#[derive(Clone, Default)]
struct CacheMetrics {
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
    changed: Repeated<()>,

    response_delay: RefCell<Option<Rc<Delay<T>>>>,
    request_delay: RefCell<Option<Rc<Delay<T>>>>,

    dev_rx: RefCell<Option<InPort<T>>>,
    mem_rx: RefCell<Option<InPort<T>>>,

    bw_bytes_per_tick: usize,

    // Internal ports
    req: RefCell<Option<OutPort<T>>>,
    rsp_arb_0: RefCell<Option<OutPort<T>>>,
    rsp_arb_1: RefCell<Option<OutPort<T>>>,
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
        let bw_bytes_per_tick = config.bw_bytes_per_tick;
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

        let request_delay_aka = build_aka!(aka, &entity, &[("mem_tx", "tx")]);
        let request_delay = Delay::new_and_register_with_renames(
            engine,
            clock,
            &entity,
            "req_delay",
            Some(&request_delay_aka),
            config.delay_ticks,
        );

        response_arbiter
            .connect_port_tx(response_delay.port_rx())
            .expect("Internal ports should connect without error");

        // Create internal ports that are driven by the cache logic
        let mut req = OutPort::new(&entity, "req_arb_0");
        req.connect(request_delay.port_rx())
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
            entity,
            clock: clock.clone(),
            spawner,
            metrics: Rc::new(RefCell::new(CacheMetrics::default())),
            contents: Rc::new(RefCell::new(CacheContents::new(config))),
            changed: Repeated::default(),
            response_delay: RefCell::new(Some(response_delay)),
            request_delay: RefCell::new(Some(request_delay)),
            dev_rx: RefCell::new(Some(dev_rx)),
            mem_rx: RefCell::new(Some(mem_rx)),
            bw_bytes_per_tick,

            req: RefCell::new(Some(req)),
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

struct RxHandlingState<T>
where
    T: SimObject + CacheAccess,
{
    entity: Rc<Entity>,
    rx: InPort<T>,
    clock: Clock,
    contents: Rc<RefCell<CacheContents<T>>>,
    metrics: Rc<RefCell<CacheMetrics>>,
    changed: Repeated<()>,
    bw_bytes_per_tick: usize,
}

#[async_trait(?Send)]
impl<T> Runnable for Cache<T>
where
    T: SimObject + CacheAccess,
{
    async fn run(&self) -> SimResult {
        {
            // Spawn a worker to handle requests from the device side
            let state = RxHandlingState {
                entity: self.entity.clone(),
                rx: take_option!(self.dev_rx),
                clock: self.clock.clone(),
                contents: self.contents.clone(),
                metrics: self.metrics.clone(),
                changed: self.changed.clone(),
                bw_bytes_per_tick: self.bw_bytes_per_tick,
            };
            let req = take_option!(self.req);
            let rsp_arb_1 = take_option!(self.rsp_arb_1);
            self.spawner
                .spawn(async move { run_dev_rx(state, req, rsp_arb_1).await });
        }

        // Handle responses from the memory side
        let state = RxHandlingState {
            entity: self.entity.clone(),
            rx: take_option!(self.mem_rx),
            clock: self.clock.clone(),
            contents: self.contents.clone(),
            metrics: self.metrics.clone(),
            changed: self.changed.clone(),
            bw_bytes_per_tick: self.bw_bytes_per_tick,
        };
        let rsp_arb_0 = take_option!(self.rsp_arb_0);
        run_mem_rx(state, rsp_arb_0).await
    }
}

async fn run_dev_rx<T>(
    mut state: RxHandlingState<T>,
    mut req: OutPort<T>,
    mut rsp_arb_1: OutPort<T>,
) -> SimResult
where
    T: SimObject + CacheAccess,
{
    loop {
        let request = state.rx.get()?.await;
        trace!(state.entity ; "Device request {}", request);
        let total_bytes = request.total_bytes();
        handle_request(&state, &mut req, &mut rsp_arb_1, request).await?;
        let ticks = total_bytes.div_ceil(state.bw_bytes_per_tick);
        state.clock.wait_ticks(ticks as u64)?.await;
    }
}

async fn run_mem_rx<T>(mut state: RxHandlingState<T>, mut rsp: OutPort<T>) -> SimResult
where
    T: SimObject + CacheAccess,
{
    loop {
        let response = state.rx.get()?.await;
        trace!(state.entity ; "Memory response {}", response);
        let total_bytes = response.total_bytes();
        let ticks = total_bytes.div_ceil(state.bw_bytes_per_tick);
        state.clock.wait_ticks(ticks as u64)?.await;
        handle_response(&state, &mut rsp, response).await?;
    }
}

async fn handle_request<T>(
    state: &RxHandlingState<T>,
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
            state
                .contents
                .borrow_mut()
                .invalidate_range(address, request.access_size_bytes())?;
        }
        AccessType::ReadRequest => {
            // Validate the whole original range before any admission or
            // metrics.
            CacheContents::<T>::checked_range(address, request.access_size_bytes())?;
            loop {
                if state.contents.borrow().has_parent_capacity() {
                    break;
                }
                state.changed.listen().await;
            }
            state.metrics.borrow_mut().payload_bytes_read += request.access_size_bytes();
            let (parent, mut cursor, end) =
                state.contents.borrow_mut().admit_read_request(request)?;
            while cursor < end {
                let action = state.contents.borrow_mut().process_line(
                    parent,
                    cursor,
                    end,
                    &mut state.metrics.borrow_mut(),
                )?;
                match action {
                    LineAction::Advance(next) => cursor = next,
                    LineAction::Send(line_read_request, next) => {
                        let ticks = line_read_request
                            .total_bytes()
                            .div_ceil(state.bw_bytes_per_tick);
                        req.put(line_read_request)?.await;
                        state.clock.wait_ticks(ticks as u64)?.await;
                        cursor = next;
                    }
                    LineAction::Blocked => state.changed.listen().await,
                }
            }
            let ready = state
                .contents
                .borrow_mut()
                .finish_issuing(parent, &mut state.metrics.borrow_mut());
            if let Some(parent) = ready {
                state.changed.notify();
                serve_parent(state, rsp, parent).await?;
            }
        }
        AccessType::WriteRequest | AccessType::WriteNonPostedRequest => {
            CacheContents::<T>::checked_range(address, request.access_size_bytes())?;
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
        other => {
            return sim_error!(
                "{}: unsupported AccessType from device: {other}",
                state.entity
            );
        }
    }
    Ok(())
}

async fn handle_response<T>(
    state: &RxHandlingState<T>,
    rsp: &mut OutPort<T>,
    response: T,
) -> SimResult
where
    T: SimObject + CacheAccess,
{
    match response.access_type() {
        AccessType::Control => {}
        AccessType::WriteNonPostedResponse => {
            state.metrics.borrow_mut().downstream_response_header_bytes += response.total_bytes();
            serve_response(state, rsp, response).await?;
        }
        AccessType::ReadResponse => {
            state.metrics.borrow_mut().downstream_response_header_bytes += response
                .total_bytes()
                .checked_sub(response.access_size_bytes())
                .ok_or_else(|| SimError("read response shorter than payload".into()))?;
            let ready = state.contents.borrow_mut().response(&response)?;
            state.changed.notify();
            for parent in ready {
                serve_parent(state, rsp, parent).await?;
            }
        }
        other => return sim_error!("{}: unsupported {other} on response port", state.entity),
    }
    Ok(())
}

async fn serve_parent<T>(state: &RxHandlingState<T>, tx: &mut OutPort<T>, request: T) -> SimResult
where
    T: SimObject + CacheAccess,
{
    let response = request.to_response(state.contents.as_ref())?;
    serve_response(state, tx, response).await
}

async fn serve_response<T>(
    state: &RxHandlingState<T>,
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
        let ticks = u64::try_from(response.total_bytes().div_ceil(state.bw_bytes_per_tick))
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
