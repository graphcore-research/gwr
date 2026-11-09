// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::model::MemoryModel;
use gwr_platform::instances::MemoryInstance;
use gwr_track::tracker::dev_null_tracker;

#[test]
fn erased_and_detailed_instances_share_metadata_without_inventing_a_concrete_handle() {
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let memory = Memory::<MemoryAccess>::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "ram",
        MemoryConfig::new(4096, 1024, 32, 2),
    )
    .unwrap();
    let detailed = MemoryInstance::Detailed(memory.clone());
    let model: Rc<dyn MemoryModel> = memory;
    let erased = MemoryInstance::Model(model);
    for instance in [&detailed, &erased] {
        assert_eq!(instance.base_address(), 4096);
        assert_eq!(instance.capacity_bytes(), 1024);
        assert_eq!(instance.bytes_read(), 0);
        drop(instance.endpoint());
    }
    assert!(detailed.detailed().is_ok());
    assert!(erased.detailed().is_err());
    assert!(detailed.detailed().unwrap().port_rx().is_ok());
}
