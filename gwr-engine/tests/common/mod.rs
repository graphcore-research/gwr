// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Wake, Waker};

use gwr_engine::engine::Engine;
use gwr_engine::events::once::Once;

struct WakeCounter {
    wakes: Arc<AtomicUsize>,
}

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
    }
}

#[must_use]
pub fn counting_waker() -> (Arc<AtomicUsize>, Waker) {
    let wakes = Arc::new(AtomicUsize::new(0));
    let waker = Waker::from(Arc::new(WakeCounter {
        wakes: wakes.clone(),
    }));
    (wakes, waker)
}

#[must_use]
pub fn wake_count(wakes: &Arc<AtomicUsize>) -> usize {
    wakes.load(Ordering::SeqCst)
}

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
