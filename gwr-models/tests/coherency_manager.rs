// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_engine::engine::Engine;
use gwr_engine::types::DeviceId;
use gwr_models::cache::coherency_manager::{CoherencyManager, CoherencyManagerConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::MemoryMap;

const DIRECTORY_DEVICE_ID: DeviceId = DeviceId(1);

#[test]
fn invalid_manager_configuration_fails_before_registration() {
    // Prepare an invalid manager configuration with a zero line size.
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let config = CoherencyManagerConfig::new(0, DIRECTORY_DEVICE_ID, MemoryMap::new());
    let clock = engine.default_clock();

    // Reject configuration before registering a runnable component.
    assert!(
        CoherencyManager::<MemoryAccess>::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "invalid",
            config
        )
        .is_err()
    );
}
