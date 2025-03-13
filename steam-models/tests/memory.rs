// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cmp::max;
use std::fmt::Display;
use std::sync::Arc;

use steam_components::sink::Sink;
use steam_components::source::Source;
use steam_components::{connect_port, option_box_repeat};
use steam_engine::engine::Engine;
use steam_engine::run_simulation;
use steam_engine::test_helpers::start_test;
use steam_engine::traits::{Routable, SimObject, TotalBytes};
use steam_engine::types::ReqType;
use steam_models::memory::{CacheHintType, Memory, MemoryAccess, MemoryConfig};
use steam_track::entity::Entity;
use steam_track::tag::Tagged;
use steam_track::{Tag, create_tag};

const BASE_ADDRESS: u64 = 0x80000;
const CAPACITY_BYTES: u64 = 0x40000;
const BW_BYTES_PER_CYCLE: u64 = 32;
const DELAY_TICKS: usize = 8;
const ACCESS_BYTES: usize = 128;

const CYCLES_PER_ACCESS: u64 = (ACCESS_BYTES as u64).div_ceil(BW_BYTES_PER_CYCLE);

#[derive(Clone, Debug)]
struct TestMemoryAccess {
    created_by: Arc<Entity>,
    tag: Tag,
    access_type: ReqType,
    num_bytes: usize,
    address: u64,
    cache_hint: CacheHintType,
}

impl Display for TestMemoryAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}: {}@{}",
            self.access_type, self.num_bytes, self.address
        )
    }
}

impl TotalBytes for TestMemoryAccess {
    fn total_bytes(&self) -> usize {
        self.num_bytes
    }
}

impl Tagged for TestMemoryAccess {
    fn tag(&self) -> Tag {
        self.tag
    }
}

impl MemoryAccess for TestMemoryAccess {
    fn access_type(&self) -> ReqType {
        self.access_type
    }

    fn addr(&self) -> u64 {
        self.address
    }

    fn cache_hint(&self) -> CacheHintType {
        CacheHintType::Allocate
    }

    fn num_bytes(&self) -> u64 {
        self.num_bytes as u64
    }

    fn to_response(&self, _mem: &impl steam_models::memory::MemoryRead) -> Self {
        TestMemoryAccess {
            created_by: self.created_by.clone(),
            tag: create_tag!(self.created_by),
            access_type: ReqType::Write,
            num_bytes: self.num_bytes,
            address: self.address,
            cache_hint: self.cache_hint,
        }
    }
}

impl Routable for TestMemoryAccess {
    fn dest(&self) -> u64 {
        self.address
    }
    fn req_type(&self) -> ReqType {
        match self.access_type {
            ReqType::Read => ReqType::Read,
            ReqType::Write => ReqType::Write,
            ReqType::WriteNonPosted => ReqType::WriteNonPosted,
            ReqType::Control => ReqType::Control,
        }
    }
}

impl TestMemoryAccess {
    fn new(created_by: &Arc<Entity>, access_type: ReqType) -> Self {
        Self {
            created_by: created_by.clone(),
            tag: create_tag!(created_by),
            num_bytes: ACCESS_BYTES,
            access_type,
            address: BASE_ADDRESS,
            cache_hint: CacheHintType::Allocate,
        }
    }
}

impl SimObject for TestMemoryAccess {}

fn create_read(created_by: &Arc<Entity>) -> TestMemoryAccess {
    TestMemoryAccess::new(created_by, ReqType::Read)
}

fn create_write(created_by: &Arc<Entity>) -> TestMemoryAccess {
    TestMemoryAccess::new(created_by, ReqType::Write)
}

fn create_write_np(created_by: &Arc<Entity>) -> TestMemoryAccess {
    TestMemoryAccess::new(created_by, ReqType::WriteNonPosted)
}

fn setup_system(
    num_accesses: usize,
    create_fn: fn(&Arc<Entity>) -> TestMemoryAccess,
) -> (
    Engine,
    Source<TestMemoryAccess>,
    Sink<TestMemoryAccess>,
    Memory<TestMemoryAccess>,
) {
    let mut engine = start_test(file!());
    let spawner = engine.spawner();
    let clock = engine.default_clock();

    let source = Source::new(engine.top(), "source", None);
    let to_put = create_fn(&source.entity);
    source.set_generator(option_box_repeat!(to_put ; num_accesses));

    let config = MemoryConfig::new(
        BASE_ADDRESS,
        CAPACITY_BYTES,
        BW_BYTES_PER_CYCLE,
        DELAY_TICKS,
    );
    let memory = Memory::new(engine.top(), "store", clock, spawner, config);
    let sink = Sink::new(engine.top(), "sink");

    connect_port!(source, tx => memory, rx);
    connect_port!(memory, tx => sink, rx);

    (engine, source, sink, memory)
}

#[test]
fn memory_read() {
    let num_accesses = 100;
    let (mut engine, source, sink, memory) = setup_system(num_accesses, create_read);

    run_simulation!(engine; [source, memory, sink]);
    assert_eq!(sink.num_sunk(), num_accesses);
    assert_eq!(memory.bytes_read(), (num_accesses * ACCESS_BYTES) as u64);
    assert_eq!(memory.bytes_written(), 0);

    let last_bw_limit_event = CYCLES_PER_ACCESS * num_accesses as u64;
    let last_packet_ack = CYCLES_PER_ACCESS * ((num_accesses - 1) as u64) + DELAY_TICKS as u64;
    let last_event_time = max(last_bw_limit_event, last_packet_ack);
    assert_eq!(engine.time_now_ns(), last_event_time as f64);
}

#[test]
fn memory_write() {
    let num_accesses = 100;
    let (mut engine, source, sink, memory) = setup_system(num_accesses, create_write);

    run_simulation!(engine; [source, memory, sink]);
    assert_eq!(sink.num_sunk(), 0);
    assert_eq!(memory.bytes_written(), (num_accesses * ACCESS_BYTES) as u64);
    assert_eq!(memory.bytes_read(), 0);

    // Simulation will only complete once the Memory has finished handling all the
    // delay imposed by the data it is carrying
    let last_bw_limit_event = CYCLES_PER_ACCESS * num_accesses as u64;
    let last_event_time = last_bw_limit_event;
    assert_eq!(engine.time_now_ns(), last_event_time as f64);
}

#[test]
fn memory_write_np() {
    let num_accesses = 100;
    let (mut engine, source, sink, memory) = setup_system(num_accesses, create_write_np);

    run_simulation!(engine; [source, memory, sink]);

    assert_eq!(sink.num_sunk(), num_accesses);
    assert_eq!(memory.bytes_written(), (num_accesses * ACCESS_BYTES) as u64);
    assert_eq!(memory.bytes_read(), 0);

    let last_bw_limit_event = CYCLES_PER_ACCESS * num_accesses as u64;
    let last_packet_ack = CYCLES_PER_ACCESS * ((num_accesses - 1) as u64) + DELAY_TICKS as u64;
    let last_event_time = max(last_bw_limit_event, last_packet_ack);
    assert_eq!(engine.time_now_ns(), last_event_time as f64);
}
