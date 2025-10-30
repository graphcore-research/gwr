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
//!  |          Device          |
//!  ----------------------------
//!       |               |
//!       |               |
//!  ----------------------------
//!  |  dev_rx          dev_tx  |
//!  |    |               ^     |
//!  |    |               |     |
//!  |    |        0  response  |
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

use async_trait::async_trait;
use gwr_components::arbiter::Arbiter;
use gwr_components::arbiter::policy::RoundRobin;
use gwr_components::delay::Delay;
use gwr_components::{connect_tx, port_rx, take_option};
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Runnable, SimObject};
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_model_builder::EntityDisplay;
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;
use gwr_track::{build_aka, trace};

#[cfg(test)]
use crate::memory::memory_access::MemoryAccess;
use crate::memory::traits::{AccessMemory, ReadMemory};

type Tag = u64;
type Index = usize;

#[derive(Clone)]
pub struct CacheConfig {
    line_size_bytes: usize,
    bw_bytes_per_cycle: usize,
    num_sets: usize,
    num_ways: usize,
    delay_ticks: usize,
}

impl CacheConfig {
    #[must_use]
    pub fn new(
        line_size_bytes: usize,
        bw_bytes_per_cycle: usize,
        num_sets: usize,
        num_ways: usize,
        delay_ticks: usize,
    ) -> Self {
        Self {
            line_size_bytes,
            bw_bytes_per_cycle,
            num_sets,
            num_ways,
            delay_ticks,
        }
    }
}

#[derive(Clone, Default)]
struct CacheMetrics {
    payload_bytes_read: usize,
    payload_bytes_written: usize,
    num_hits: usize,
    num_misses: usize,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
enum EntryState {
    #[default]
    Available,
    Allocated,
    ValidData,
}

#[derive(Default, Clone)]
struct CacheEntry {
    state: EntryState,
    tag: Tag,
}

// Cache structure:
//  A set comprises N-ways
type Set = Vec<CacheEntry>;
//  The cache comprises M-sets
type Sets = Vec<Set>;

struct CacheContents<T>
where
    T: SimObject + AccessMemory,
{
    config: CacheConfig,
    sets: Sets,
    waiting_for_response: Vec<(Tag, Index, T)>,
    lru_indices: Vec<usize>,
}

impl<T> CacheContents<T>
where
    T: SimObject + AccessMemory,
{
    fn new(config: CacheConfig) -> Self {
        let sets = vec![vec![CacheEntry::default(); config.num_ways]; config.num_sets];
        let lru_indices = vec![0; config.num_sets];
        Self {
            config,
            sets,
            waiting_for_response: Vec::new(),
            lru_indices,
        }
    }

    /// Split up an address into its component parts:
    ///
    ///  msb                  lsb
    ///  +-----+-------+--------+
    ///  | tag | index | offset |
    ///  +-----+-------+--------+
    ///
    /// Where:
    ///  - offset within a cache line
    ///  - index is the part of the address used to select a cache set (n-ways)
    ///  - tag contains the rest of the address that is compared to determine
    ///    address matches
    fn tag_and_index_for_addr(&self, addr: u64) -> (Tag, Index) {
        let index = (addr as usize / self.config.line_size_bytes) % self.config.num_sets;
        let tag = addr / self.config.line_size_bytes as u64 / self.config.num_sets as u64;
        (tag, index)
    }

    fn state_for(&self, addr: u64) -> Option<EntryState> {
        let (tag, index) = self.tag_and_index_for_addr(addr);
        for i in 0..self.config.num_ways {
            if (self.sets[index][i].state != EntryState::Available)
                && self.sets[index][i].tag == tag
            {
                return Some(self.sets[index][i].state);
            }
        }
        None
    }

    fn allocate(&mut self, addr: u64) {
        let (tag, index) = self.tag_and_index_for_addr(addr);

        let insert_index = self.lru_indices[index];
        self.lru_indices[index] = (self.lru_indices[index] + 1) % self.config.num_ways;

        self.sets[index][insert_index].tag = tag;
        self.sets[index][insert_index].state = EntryState::Allocated;
    }

    fn set_data_valid(&mut self, addr: u64) {
        let (tag, index) = self.tag_and_index_for_addr(addr);

        for i in 0..self.config.num_ways {
            if (self.sets[index][i].state != EntryState::Available)
                && self.sets[index][i].tag == tag
            {
                self.sets[index][i].state = EntryState::ValidData;
                break;
            }
        }
    }

    fn invalidate(&mut self, addr: u64) {
        let (tag, index) = self.tag_and_index_for_addr(addr);

        for i in 0..self.config.num_ways {
            if self.sets[index][i].tag == tag {
                self.sets[index][i].state = EntryState::Available;
                self.sets[index][i].tag = 0;
                break;
            }
        }
    }

    fn add_waiting_for_response(&mut self, request: T) {
        let (tag, index) = self.tag_and_index_for_addr(request.destination());
        self.waiting_for_response.push((tag, index, request));
    }

    fn get_requests_waiting_for_response(&mut self, response: &T) -> Option<Vec<T>> {
        // The `source` on the response is the original `destination`
        let (response_tag, response_index) = self.tag_and_index_for_addr(response.source());

        // If there are any requests waiting for this response then return matching sets
        if self
            .waiting_for_response
            .iter()
            .any(|(tag, index, _x)| *tag == response_tag && *index == response_index)
        {
            let all: Vec<(Tag, Index, T)> = self.waiting_for_response.drain(..).collect();
            let (matching, not_matching) = all
                .into_iter()
                .partition(|(tag, index, _x)| *tag == response_tag && *index == response_index);
            self.waiting_for_response = not_matching;
            let matching = matching.into_iter().map(|(_, _, x)| x).collect();
            Some(matching)
        } else {
            None
        }
    }
}

impl<T> ReadMemory for CacheContents<T>
where
    T: SimObject + AccessMemory,
{
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

impl<T> ReadMemory for RefCell<CacheContents<T>>
where
    T: SimObject + AccessMemory,
{
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

#[derive(EntityDisplay)]
pub struct Cache<T>
where
    T: SimObject + AccessMemory,
{
    pub entity: Rc<Entity>,

    clock: Clock,
    spawner: Spawner,
    metrics: Rc<RefCell<CacheMetrics>>,
    contents: Rc<RefCell<CacheContents<T>>>,

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
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        config: CacheConfig,
    ) -> Result<Rc<Self>, SimError> {
        let bw_bytes_per_cycle = config.bw_bytes_per_cycle;
        let entity = Rc::new(Entity::new(parent, name));

        let policy = Box::new(RoundRobin::new());
        let response_arbiter_aka = build_aka!(aka, &entity, &[("dev_tx", "tx")]);
        let response_arbiter = Arbiter::new_and_register_with_renames(
            engine,
            clock,
            &entity,
            "rsp_arb",
            Some(&response_arbiter_aka),
            2,
            policy,
        )?;

        let response_delay =
            Delay::new_and_register(engine, clock, &entity, "rsp_delay", config.delay_ticks)?;

        let request_delay_aka = build_aka!(aka, &entity, &[("mem_tx", "tx")]);
        let request_delay = Delay::new_and_register_with_renames(
            engine,
            clock,
            &entity,
            "req_delay",
            Some(&request_delay_aka),
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

        let dev_rx = InPort::new_with_renames(engine, clock, &entity, "dev_rx", aka);
        let mem_rx = InPort::new_with_renames(engine, clock, &entity, "mem_rx", aka);

        let spawner = engine.spawner();
        let rc_self = Rc::new(Self {
            entity,
            clock: clock.clone(),
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
}

struct RxHandlingState<T>
where
    T: SimObject + AccessMemory,
{
    entity: Rc<Entity>,
    rx: InPort<T>,
    clock: Clock,
    contents: Rc<RefCell<CacheContents<T>>>,
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
        let total_bytes = request.total_bytes();
        handle_request(state, &req, &rsp_arb_1, request).await?;
        let ticks = total_bytes.div_ceil(state.bw_bytes_per_cycle);
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
    let addr = request.destination();
    match request.access_type() {
        AccessType::Control => {
            state.contents.borrow_mut().invalidate(addr);
        }
        AccessType::Read => {
            state.metrics.borrow_mut().payload_bytes_read += request.access_size_bytes();
            let line_state = state.contents.borrow().state_for(addr);
            match line_state {
                Some(EntryState::ValidData) => {
                    let response = request.to_response(state.contents.as_ref());
                    rsp_arb_1.put(response)?.await;
                    state.metrics.borrow_mut().num_hits += 1;
                }
                Some(EntryState::Allocated) => {
                    // There is an outstanding request to memory for this address already
                    state
                        .contents
                        .borrow_mut()
                        .add_waiting_for_response(request);
                    state.metrics.borrow_mut().num_hits += 1;
                }
                Some(EntryState::Available) | None => {
                    state.contents.borrow_mut().allocate(addr);
                    req.put(request)?.await;
                    state.metrics.borrow_mut().num_misses += 1;
                }
            }
        }

        AccessType::Write | AccessType::WriteNonPosted => {
            state.metrics.borrow_mut().payload_bytes_written += request.access_size_bytes();
            state.contents.borrow_mut().invalidate(addr);
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
        let total_bytes = response.total_bytes();
        handle_response(state, &rsp_arb_0, response).await?;
        let ticks = total_bytes.div_ceil(state.bw_bytes_per_cycle);
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
    match access.access_type() {
        AccessType::Control => {
            // Drop and ignore for now
        }
        AccessType::Read => {
            return sim_error!(format!("{}: read on response port", state.entity));
        }
        AccessType::Write => {
            state.contents.borrow_mut().set_data_valid(access.source());
            let matching = state
                .contents
                .borrow_mut()
                .get_requests_waiting_for_response(&access);

            // Forward this response back to the memory (via the arbiter)
            rsp_arb_0.put(access)?.await;

            // Forward on
            if let Some(m) = matching {
                for x in m {
                    let response = x.to_response(state.contents.as_ref());
                    rsp_arb_0.put(response)?.await;
                }
            }
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
    let num_sets = 1024;
    let num_ways = 4;
    let config = CacheConfig::new(line_size_bytes, bw_bytes_per_cycle, num_sets, num_ways, 8);
    let mut state: CacheContents<MemoryAccess> = CacheContents::new(config);

    let mut addrs = Vec::new();
    let mut addr = 0x1000000;
    for _ in 0..num_ways + 1 {
        addrs.push(addr);
        addr += (line_size_bytes * num_sets * num_ways) as u64;
    }

    for addr in addrs.iter().take(num_ways) {
        assert_eq!(state.state_for(*addr), None);
        state.allocate(*addr);
        assert_eq!(state.state_for(*addr), Some(EntryState::Allocated));
    }

    state.allocate(addrs[num_ways]);

    // Should have been evicted
    assert_eq!(state.state_for(addrs[0]), None);
    for i in 0..num_ways {
        // While all the rest remain
        assert_eq!(state.state_for(addrs[i + 1]), Some(EntryState::Allocated));
    }
}

#[test]
fn invalidate() {
    let num_ways = 4;
    let config = CacheConfig::new(32, 32, 1024, num_ways, 8);
    let mut state: CacheContents<MemoryAccess> = CacheContents::new(config);

    let addr = 0x40000;
    state.allocate(addr);
    assert_eq!(state.state_for(addr), Some(EntryState::Allocated));
    state.invalidate(addr);
    assert_eq!(state.state_for(addr), None);
}
