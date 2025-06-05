// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

/// Benchmark basic component usage.
use criterion::{BatchSize, Criterion, criterion_group};
use steam_components::arbiter::{Arbiter, Priority, RoundRobinPolicy};
use steam_components::delay::Delay;
use steam_components::flow_controls::limiter::Limiter;
use steam_components::sink::Sink;
use steam_components::source::Source;
use steam_components::store::Store;
use steam_components::test_helpers::{ArbiterInputData, priority_policy_test_core};
use steam_components::{connect_port, option_box_repeat, rc_limiter};
use steam_engine::engine::Engine;
use steam_engine::spawn_simulation;
use steam_track::tracker::dev_null_tracker;

fn create_engine() -> Engine {
    // Create an engine without the tracker system opening files for logging
    let tracker = dev_null_tracker();
    Engine::new(&tracker)
}

fn run_engine(mut engine: Engine) {
    engine.run().unwrap();
}

fn spawn_source_store_sink() -> Engine {
    let engine = create_engine();
    let spawner = engine.spawner();

    let capacity = 5;
    let num_puts = 200;

    let source = Source::new(engine.top(), "source", option_box_repeat!(1 ; num_puts));
    let store = Store::new(engine.top(), "store", spawner, capacity);
    let sink = Sink::new(engine.top(), "sink");

    connect_port!(source, tx => store, rx);
    connect_port!(store, tx => sink, rx);

    spawn_simulation!(engine ; [source, store, sink]);
    engine
}

fn spawn_source_delay_sink() -> Engine {
    let mut engine = create_engine();
    let clock = engine.default_clock();

    let delay = 3;
    let num_puts = 200;

    let spawner = engine.spawner();

    let source = Source::new(engine.top(), "source", option_box_repeat!(500; num_puts));
    let delay = Delay::new(engine.top(), "delay", clock, spawner, delay);
    let sink = Sink::new(engine.top(), "sink");

    connect_port!(source, tx => delay, rx);
    connect_port!(delay, tx => sink, rx);

    spawn_simulation!(engine ; [source, delay, sink]);
    engine
}

fn spawn_arbiter_fixedpriority_policy() -> Engine {
    let mut engine = create_engine();
    let inputs = vec![
        ArbiterInputData {
            val: 1,
            count: 1000,
            weight: 0,
            priority: Priority::Low,
        },
        ArbiterInputData {
            val: 2,
            count: 1500,
            weight: 0,
            priority: Priority::Medium,
        },
        ArbiterInputData {
            val: 3,
            count: 1000,
            weight: 0,
            priority: Priority::Medium,
        },
        ArbiterInputData {
            val: 4,
            count: 1500,
            weight: 0,
            priority: Priority::Low,
        },
    ];

    priority_policy_test_core(&mut engine, &inputs);
    engine
}

fn spawn_larger_simulation() -> Engine {
    let mut engine = create_engine();
    let spawner = engine.spawner();
    let clock = engine.default_clock();
    let rate_limiter = rc_limiter!(clock, 1);

    let num_puts = 20;

    let num_sources = 5000;
    let mut sources = Vec::new();

    let mut arbiter = Arbiter::new(
        engine.top(),
        "arb",
        spawner,
        num_sources,
        Box::new(RoundRobinPolicy::new()),
    );
    let sink_limiter = Limiter::new(engine.top(), "limit_sink", rate_limiter);
    let sink = Sink::new(engine.top(), "sink");
    for i in 0..num_sources {
        sources.push(Source::new(
            engine.top(),
            format!("source{}", i).as_str(),
            option_box_repeat!(i ; num_puts),
        ));
        connect_port!(sources[i], tx => arbiter, rx, i);
    }

    connect_port!(arbiter, tx => sink_limiter, rx);
    connect_port!(sink_limiter, tx => sink, rx);

    spawn_simulation!(engine ; sources, [arbiter, sink, sink_limiter]);
    engine
}

fn bench_blocks(c: &mut Criterion) {
    let mut group = c.benchmark_group("blocks");

    group.bench_function("source_store_sink", |b| {
        b.iter_batched(spawn_source_store_sink, run_engine, BatchSize::SmallInput);
    });

    group.bench_function("source_delay_sink", |b| {
        b.iter_batched(spawn_source_delay_sink, run_engine, BatchSize::SmallInput);
    });

    group.bench_function("arbiter_fixedpriority_policy", |b| {
        b.iter_batched(
            spawn_arbiter_fixedpriority_policy,
            run_engine,
            BatchSize::SmallInput,
        );
    });

    group.bench_function("larger_simulation", |b| {
        b.iter_batched(spawn_larger_simulation, run_engine, BatchSize::SmallInput);
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = bench_blocks
}
