// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! A basic n-way associative cache model.
//!
//! This is currently just a proof of concept that the ports can be created with
//! different names that handle the same data type. This was not possible with
//! the multi-threaded / async-trait approach.
//!
//! The basic wiring of requests should be working, and the core tag / way
//! management has simple tests.
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
//!  |    |         0 response  |
//!  |    +--delay--> arbiter   |
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
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use steam_components::arbiter::{Arbiter, RoundRobinPolicy};
use steam_components::delay::Delay;
use steam_components::{connect_tx, port_rx, take_option};
use steam_engine::engine::Engine;
use steam_engine::executor::Spawner;
use steam_engine::port::{InPort, OutPort, PortStateResult};
use steam_engine::sim_error;
use steam_engine::time::clock::Clock;
use steam_engine::traits::{Runnable, SimObject};
use steam_engine::types::{AccessType, SimError, SimResult};
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;
use steam_track::trace;

use crate::memory::{AccessMemory, MemoryRead};

type Tag = u64;

#[derive(Clone)]
pub struct CacheConfig {
    line_size_bytes: usize,
    bw_bytes_per_cycle: usize,
    num_lines: usize,
    num_ways: usize,
    delay_ticks: usize,
}

impl CacheConfig {
    #[must_use]
    pub fn new(
        line_size_bytes: usize,
        bw_bytes_per_cycle: usize,
        num_lines: usize,
        num_ways: usize,
        delay_ticks: usize,
    ) -> Self {
        Self {
            line_size_bytes,
            bw_bytes_per_cycle,
            num_lines,
            num_ways,
            delay_ticks,
        }
    }
}

#[derive(Clone, Default)]
struct CacheMetrics {
    bytes_read: usize,
    bytes_written: usize,
    num_hits: usize,
    num_misses: usize,
}

type TagWays = Vec<Tag>;
type TagEntries = Vec<TagWays>;
type ValidWays = Vec<bool>;
type ValidEntries = Vec<ValidWays>;

struct CacheContents {
    num_entries: usize,
    config: CacheConfig,
    valid: ValidEntries,
    tags: TagEntries,
    lru_indices: Vec<usize>,
}

impl CacheContents {
    fn new(config: CacheConfig) -> Self {
        let num_entries = config.num_lines / config.num_ways;
        let valid = vec![vec![false; config.num_ways]; num_entries];
        let tags = vec![vec![0; config.num_ways]; num_entries];
        let lru_indices = vec![0; num_entries];
        Self {
            num_entries,
            config,
            valid,
            tags,
            lru_indices,
        }
    }

    fn entry_for(&self, addr: u64) -> (Tag, usize) {
        let tag = addr / self.config.line_size_bytes as Tag;
        (tag, tag as usize % self.num_entries)
    }

    fn contains(&self, addr: u64) -> bool {
        let (tag, index) = self.entry_for(addr);
        for i in 0..self.config.num_ways {
            if self.valid[index][i] && self.tags[index][i] == tag {
                return true;
            }
        }
        false
    }

    fn insert(&mut self, addr: u64) {
        let (tag, index) = self.entry_for(addr);

        let insert_index = self.lru_indices[index];
        self.lru_indices[index] = (self.lru_indices[index] + 1) % self.config.num_ways;

        self.tags[index][insert_index] = tag;
        self.valid[index][insert_index] = true;
    }

    fn invalidate(&mut self, addr: u64) {
        let (tag, index) = self.entry_for(addr);
        for i in 0..self.config.num_ways {
            if self.tags[index][i] == tag {
                self.valid[index][i] = false;
                self.tags[index][i] = 0;
                break;
            }
        }
    }
}

impl MemoryRead for CacheContents {
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

impl MemoryRead for RefCell<CacheContents> {
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

#[derive(EntityDisplay)]
pub struct Cache<T>
where
    T: SimObject + AccessMemory,
{
    pub entity: Arc<Entity>,

    clock: Clock,
    spawner: Spawner,
    metrics: Rc<RefCell<CacheMetrics>>,
    contents: Rc<RefCell<CacheContents>>,

    response_arbiter: RefCell<Option<Rc<Arbiter<T>>>>,

    request_delay: RefCell<Option<Rc<Delay<T>>>>,

    dev_rx: RefCell<Option<InPort<T>>>,
    mem_rx: RefCell<Option<InPort<T>>>,

    bw_bytes_per_cycle: usize,

    // Internal ports
    req: RefCell<Option<OutPort<T>>>,
    rsp_arb_0: RefCell<Option<OutPort<T>>>,
    rsp_arb_1: RefCell<Option<OutPort<T>>>,
}

impl<T> Cache<T>
where
    T: SimObject + AccessMemory,
{
    /// Create an instance of the cache and register it with the Engine.
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        clock: Clock,
        spawner: Spawner,
        config: CacheConfig,
    ) -> Result<Rc<Self>, SimError> {
        let bw_bytes_per_cycle = config.bw_bytes_per_cycle;
        let entity = Arc::new(Entity::new(parent, name));

        let policy = Box::new(RoundRobinPolicy::new());
        let response_arbiter =
            Arbiter::new_and_register(engine, &entity, "rsp_arb", spawner.clone(), 2, policy)?;

        let response_delay = Delay::new_and_register(
            engine,
            &entity,
            "rsp_delay",
            clock.clone(),
            spawner.clone(),
            config.delay_ticks,
        )?;

        let request_delay = Delay::new_and_register(
            engine,
            &entity,
            "req_delay",
            clock.clone(),
            spawner.clone(),
            config.delay_ticks,
        )?;

        response_delay.connect_port_tx(response_arbiter.port_rx_i(0))?;

        // Create internal ports that are driven by the cache logic
        let mut req = OutPort::new(&entity, "req_arb_0");
        req.connect(request_delay.port_rx())?;

        let mut rsp_arb_0 = OutPort::new(&entity, "rsp_arb_0");
        rsp_arb_0.connect(response_delay.port_rx())?;

        let mut rsp_arb_1 = OutPort::new(&entity, "rsp_arb_1");
        rsp_arb_1.connect(response_arbiter.port_rx_i(1))?;

        let dev_rx = InPort::new(&entity, "dev_rx");
        let mem_rx = InPort::new(&entity, "mem_rx");

        let rc_self = Rc::new(Self {
            entity,
            clock,
            spawner,
            metrics: Rc::new(RefCell::new(CacheMetrics::default())),
            contents: Rc::new(RefCell::new(CacheContents::new(config))),
            response_arbiter: RefCell::new(Some(response_arbiter)),
            request_delay: RefCell::new(Some(request_delay)),
            dev_rx: RefCell::new(Some(dev_rx)),
            mem_rx: RefCell::new(Some(mem_rx)),
            bw_bytes_per_cycle,

            req: RefCell::new(Some(req)),
            rsp_arb_0: RefCell::new(Some(rsp_arb_0)),
            rsp_arb_1: RefCell::new(Some(rsp_arb_1)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn connect_port_dev_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        connect_tx!(self.response_arbiter, connect_port_tx ; port_state)
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
    pub fn bytes_written(&self) -> usize {
        self.metrics.borrow().bytes_written
    }

    #[must_use]
    pub fn bytes_read(&self) -> usize {
        self.metrics.borrow().bytes_read
    }

    #[must_use]
    pub fn num_hits(&self) -> usize {
        self.metrics.borrow().num_hits
    }

    #[must_use]
    pub fn num_misses(&self) -> usize {
        self.metrics.borrow().num_misses
    }
}

struct RxHandlingState<T>
where
    T: SimObject + AccessMemory,
{
    entity: Arc<Entity>,
    rx: InPort<T>,
    clock: Clock,
    contents: Rc<RefCell<CacheContents>>,
    metrics: Rc<RefCell<CacheMetrics>>,
    bw_bytes_per_cycle: usize,
}

#[async_trait(?Send)]
impl<T> Runnable for Cache<T>
where
    T: SimObject + AccessMemory,
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
                bw_bytes_per_cycle: self.bw_bytes_per_cycle,
            };
            let req = take_option!(self.req);
            let rsp_arb_1 = take_option!(self.rsp_arb_1);
            self.spawner
                .spawn(async move { run_dev_rx(&state, req, rsp_arb_1).await });
        }

        // Handle responses from the memory side
        let state = RxHandlingState {
            entity: self.entity.clone(),
            rx: take_option!(self.mem_rx),
            clock: self.clock.clone(),
            contents: self.contents.clone(),
            metrics: self.metrics.clone(),
            bw_bytes_per_cycle: self.bw_bytes_per_cycle,
        };
        let rsp_arb_0 = take_option!(self.rsp_arb_0);
        run_mem_rx(&state, rsp_arb_0).await
    }
}

async fn run_dev_rx<T>(
    state: &RxHandlingState<T>,
    req: OutPort<T>,
    rsp_arb_1: OutPort<T>,
) -> SimResult
where
    T: SimObject + AccessMemory,
{
    loop {
        let request = state.rx.get()?.await;
        trace!(state.entity ; "Device request {}", request);
        let access_bytes = request.num_bytes();
        handle_request(state, &req, &rsp_arb_1, request).await?;
        let ticks = access_bytes.div_ceil(state.bw_bytes_per_cycle);
        state.clock.wait_ticks(ticks as u64).await;
    }
}

async fn handle_request<T>(
    state: &RxHandlingState<T>,
    req: &OutPort<T>,
    rsp_arb_1: &OutPort<T>,
    request: T,
) -> SimResult
where
    T: SimObject + AccessMemory,
{
    let addr = request.dst_addr();
    let access_bytes = request.num_bytes();
    match request.req_type()? {
        AccessType::Control => {
            state.contents.borrow_mut().invalidate(addr);
        }
        AccessType::Read => {
            state.metrics.borrow_mut().bytes_read += access_bytes;
            if state.contents.borrow().contains(addr) {
                let response = request.to_response(state.contents.as_ref());
                rsp_arb_1.put(response)?.await;
                state.metrics.borrow_mut().num_hits += 1;
            } else {
                req.put(request)?.await;
                state.metrics.borrow_mut().num_misses += 1;
            }
        }
        AccessType::Write | AccessType::WriteNonPosted => {
            state.metrics.borrow_mut().bytes_written += access_bytes;
            {
                let mut contents = state.contents.borrow_mut();
                if contents.contains(addr) {
                    // Write hits in cache - flush contents
                    contents.invalidate(addr);
                }
            }
            req.put(request)?.await;
        }
    }

    Ok(())
}

async fn run_mem_rx<T>(state: &RxHandlingState<T>, rsp_arb_0: OutPort<T>) -> SimResult
where
    T: SimObject + AccessMemory,
{
    loop {
        let response = state.rx.get()?.await;
        trace!(state.entity ; "Memory response {}", response);
        let access_bytes = response.num_bytes();
        handle_response(state, &rsp_arb_0, response).await?;
        let ticks = access_bytes.div_ceil(state.bw_bytes_per_cycle);
        state.clock.wait_ticks(ticks as u64).await;
    }
}

async fn handle_response<T>(
    state: &RxHandlingState<T>,
    rsp_arb_0: &OutPort<T>,
    access: T,
) -> SimResult
where
    T: SimObject + AccessMemory,
{
    match access.req_type()? {
        AccessType::Control => {
            // Drop and ignore for now
        }
        AccessType::Read => {
            return sim_error!(format!("{}: read on response port", state.entity));
        }
        AccessType::Write => {
            // Store with the source address that it is handling
            state.contents.borrow_mut().insert(access.src_addr());
            rsp_arb_0.put(access)?.await;
        }
        AccessType::WriteNonPosted => {
            return sim_error!(format!(
                "{}: write non-posted on response port",
                state.entity
            ));
        }
    }

    Ok(())
}

#[test]
fn basic_ways() {
    let line_size_bytes = 32;
    let bw_bytes_per_cycle = 32;
    let num_lines = 1024;
    let num_ways = 4;
    let config = CacheConfig::new(line_size_bytes, bw_bytes_per_cycle, num_lines, num_ways, 8);
    let mut state = CacheContents::new(config);

    let mut addrs = Vec::new();
    let mut addr = 0x1000000;
    for _ in 0..num_ways + 1 {
        addrs.push(addr);
        addr += (line_size_bytes * (num_lines / num_ways)) as u64;
    }

    for addr in addrs.iter().take(num_ways) {
        assert!(!state.contains(*addr));
        state.insert(*addr);
        assert!(state.contains(*addr));
    }

    state.insert(addrs[num_ways]);

    // Should have been evicted
    assert!(!state.contains(addrs[0]));
    for i in 0..num_ways {
        // While all the rest remain
        assert!(state.contains(addrs[i + 1]));
    }
}

#[test]
fn invalidate() {
    let num_ways = 4;
    let config = CacheConfig::new(32, 32, 1024, num_ways, 8);
    let mut state = CacheContents::new(config);

    let addr = 0x40000;
    state.insert(addr);
    assert!(state.contains(addr));
    state.invalidate(addr);
    assert!(!state.contains(addr));
}
