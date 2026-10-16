// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::traits::{Routable, TotalBytes};
use gwr_models::build_model_harness;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::traits::AccessMemory;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::{MemoryTxn, create_default_memory_map, create_read};
use gwr_track::id::Unique;
use gwr_track::tracker::dev_null_tracker;

build_model_harness! {
    harness MemoryOnlyHarness<T> {
        component: memory: Rc<Memory<T>>,
        rx ports: { Rx<T> => rx },
        tx ports: { Tx<T> => tx },
    }
}

#[test]
fn ordinary_memory_transactions_do_not_require_cache_support() {
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let memory = Memory::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "memory",
        MemoryConfig::new(0x80000, 65536, 32, 20),
    )
    .unwrap();
    let map = Rc::new(create_default_memory_map());
    let request = MemoryOnly(create_read(engine.top(), &map, 1, 0x80000, 0x81000, 16));
    let mut harness = MemoryOnlyHarness::new(engine, memory);
    harness.run_steps([
        send_rx!(request),
        expect_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(1)),
    ]);
}

// Ordinary memory transactions do not need to implement CacheAccess.
#[derive(Clone, Debug)]
struct MemoryOnly(MemoryAccess);
impl std::fmt::Display for MemoryOnly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}
impl Unique for MemoryOnly {
    fn id(&self) -> gwr_track::Id {
        Unique::id(&self.0)
    }
}
impl TotalBytes for MemoryOnly {
    fn total_bytes(&self) -> usize {
        TotalBytes::total_bytes(&self.0)
    }
}
impl Routable for MemoryOnly {
    fn dst_addr(&self) -> u64 {
        Routable::dst_addr(&self.0)
    }

    fn src_addr(&self) -> u64 {
        Routable::src_addr(&self.0)
    }

    fn dst_device(&self) -> gwr_engine::types::DeviceId {
        Routable::dst_device(&self.0)
    }

    fn src_device(&self) -> gwr_engine::types::DeviceId {
        Routable::src_device(&self.0)
    }

    fn access_type(&self) -> gwr_engine::types::AccessType {
        Routable::access_type(&self.0)
    }
}
impl gwr_engine::traits::SimObject for MemoryOnly {}
impl AccessMemory for MemoryOnly {
    fn access_size_bytes(&self) -> usize {
        AccessMemory::access_size_bytes(&self.0)
    }

    fn to_response(
        &self,
        m: &impl gwr_models::memory::traits::ReadMemory,
    ) -> Result<Self, gwr_engine::types::SimError> {
        AccessMemory::to_response(&self.0, m).map(Self)
    }
}
