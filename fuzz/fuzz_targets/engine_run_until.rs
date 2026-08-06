// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#![no_main]

use gwr_engine::engine::Engine;
use gwr_engine::events::all_of::AllOf;
use gwr_engine::events::any_of::AnyOf;
use gwr_engine::events::once::Once;
use gwr_engine::types::Eventable;
use gwr_track::tracker::dev_null_tracker;
use libfuzzer_sys::{arbitrary, fuzz_target};

const MAX_EVENTS: usize = 8;

#[derive(arbitrary::Arbitrary, Debug)]
enum Operation {
    Once,
    AnyOf,
    AllOf,
}

#[derive(arbitrary::Arbitrary, Debug)]
struct FuzzInput {
    task_order_seed: u64,
    operation: Operation,
    delays: Vec<u8>,
}

// Helper function to create an event and spawn a task that will trigger it
// after the specified time.
pub fn create_once_event_at_delay<T>(engine: &mut Engine, delay: u64, value: T) -> Eventable<T>
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

fuzz_target!(|input: FuzzInput| {
    let delays = input
        .delays
        .iter()
        .take(MAX_EVENTS)
        .copied()
        .collect::<Vec<_>>();

    if delays.is_empty() {
        return;
    }

    let mut engine = Engine::new(&dev_null_tracker());

    engine.set_task_order_seed(input.task_order_seed);
    engine.set_randomize_task_order(true);

    let events: Vec<Eventable<usize>> = delays
        .iter()
        .enumerate()
        .map(|(i, &d)| create_once_event_at_delay(&mut engine, d as u64, i))
        .collect();

    let (op, expected): (Eventable<usize>, f64) = match input.operation {
        Operation::Once => (events.into_iter().next().unwrap(), delays[0] as f64),
        Operation::AnyOf => (
            Box::new(AnyOf::new(events)),
            *delays.iter().min().unwrap() as f64,
        ),
        Operation::AllOf => (
            Box::new(AllOf::new(events)),
            *delays.iter().max().unwrap() as f64,
        ),
    };

    engine.run_until(op).unwrap();

    assert_eq!(engine.time_now_ns(), expected);

    // Complete any events left pending when Once or AnyOf returned so that
    // their scheduled tasks are released before the engine is dropped.
    engine.run().unwrap();
});
