// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Ordinary caches retain their packet contract without coherence support.
use std::fmt;

use gwr_engine::engine::Engine;
use gwr_engine::traits::{Routable, SimObject, TotalBytes};
use gwr_engine::types::{AccessType, DeviceId, SimError};
use gwr_models::cache::{Cache, CacheConfig};
use gwr_models::memory::CacheHintType;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::traits::{AccessMemory, CacheAccess, CacheFillToken, ReadMemory};
use gwr_track::Id;
use gwr_track::id::Unique;

const LINE_SIZE_BYTES: usize = 32;
const BANDWIDTH_BYTES_PER_TICK: usize = 8;

#[derive(Clone, Debug)]
struct PlainPacket(MemoryAccess);

impl fmt::Display for PlainPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl TotalBytes for PlainPacket {
    fn total_bytes(&self) -> usize {
        self.0.total_bytes()
    }
}

impl Unique for PlainPacket {
    fn id(&self) -> Id {
        self.0.id()
    }
}

impl Routable for PlainPacket {
    fn dst_addr(&self) -> u64 {
        self.0.dst_addr()
    }

    fn src_addr(&self) -> u64 {
        self.0.src_addr()
    }

    fn dst_device(&self) -> DeviceId {
        self.0.dst_device()
    }

    fn src_device(&self) -> DeviceId {
        self.0.src_device()
    }

    fn access_type(&self) -> AccessType {
        self.0.access_type()
    }
}

impl AccessMemory for PlainPacket {
    fn access_size_bytes(&self) -> usize {
        self.0.access_size_bytes()
    }

    fn to_response(&self, mem: &impl ReadMemory) -> Result<Self, SimError> {
        self.0.to_response(mem).map(Self)
    }

    fn to_ack(&self) -> Result<Self, gwr_engine::types::SimError> {
        self.0.to_ack().map(Self)
    }
}

impl CacheAccess for PlainPacket {
    fn cache_hint(&self) -> CacheHintType {
        self.0.cache_hint()
    }

    fn cache_read_fragment(
        &self,
        address: u64,
        bytes: usize,
        token: CacheFillToken,
    ) -> Result<Self, SimError> {
        self.0.cache_read_fragment(address, bytes, token).map(Self)
    }

    fn cache_fill_token(&self) -> Option<&CacheFillToken> {
        self.0.cache_fill_token()
    }
}

impl SimObject for PlainPacket {}
#[test]
fn ordinary_cache_accepts_packets_without_coherent_access() {
    // Use a packet implementing CacheAccess without CoherentAccess.
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();

    // Construct an ordinary cache without requiring coherence support.
    Cache::<PlainPacket>::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "plain",
        CacheConfig::new(LINE_SIZE_BYTES, BANDWIDTH_BYTES_PER_TICK, 1, 1, 0),
    )
    .unwrap();
}
