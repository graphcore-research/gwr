// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use gwr_engine::engine::Engine;
use gwr_engine::events::once::Once;

/// Spawn an activity loop that runs continually
pub fn spawn_activity(engine: &mut Engine) {
    let clock = engine.default_clock();
    engine.spawn(async move {
        loop {
            clock.wait_ticks(1).await;
            println!("Running {}", clock.tick_now());
        }
    });
}

// Helper function to create an event and spawn a task that will trigger it
// after the specified time.
pub fn create_once_event_at_delay<T>(engine: &mut Engine, delay: u64, value: T) -> Box<Once<T>>
where
    T: Copy + 'static,
{
    let event = Once::with_value(value);
    {
        let clock = engine.default_clock();
        let event = event.clone();
        engine.spawn(async move {
            clock.wait_ticks(delay).await;
            event.notify()?;
            Ok(())
        });
    }
    Box::new(event)
}
