// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

/// Benchmark basic component usage.
use criterion::{BatchSize, Criterion, criterion_group};
use gwr_components::arbiter::Arbiter;
use gwr_components::arbiter::policy::{Priority, RoundRobin};
use gwr_components::delay::Delay;
use gwr_components::flow_controls::limiter::Limiter;
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::store::Store;
use gwr_components::test_helpers::{ArbiterInputData, priority_policy_test_core};
use gwr_components::{connect_port, option_box_repeat, rc_limiter};
use gwr_engine::engine::Engine;
use gwr_track::tracker::dev_null_tracker;

fn create_engine() -> Engine {
    // Create an engine without the tracker system opening files for logging
    let tracker = dev_null_tracker();
    Engine::new(&tracker)
}

fn run_engine(mut engine: Engine) {
    engine.run().unwrap();
}

fn spawn_source_store_sink() -> Engine {
    let mut engine = create_engine();
    let clock = engine.default_clock();

    let capacity = 5;
    let num_puts = 200;

    let top = engine.top();
    let source =
        Source::new_and_register(&engine, top, "source", option_box_repeat!(1 ; num_puts)).unwrap();
    let store = Store::new_and_register(&engine, &clock, top, "store", capacity).unwrap();
    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();

    source.connect_port_tx(store.port_rx()).unwrap();
    connect_port!(store, tx => sink, rx).unwrap();

    engine
}

fn spawn_source_delay_sink() -> Engine {
    let mut engine = create_engine();
    let clock = engine.default_clock();

    let delay = 3;
    let num_puts = 200;

    let top = engine.top();
    let source =
        Source::new_and_register(&engine, top, "source", option_box_repeat!(500; num_puts))
            .unwrap();
    let delay = Delay::new_and_register(&engine, &clock, top, "delay", delay).unwrap();
    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();

    connect_port!(source, tx => delay, rx).unwrap();
    connect_port!(delay, tx => sink, rx).unwrap();

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
    let clock = engine.default_clock();
    let rate_limiter = rc_limiter!(&clock, 1);

    let num_puts = 20;

    let num_sources = 5000;
    let mut sources = Vec::new();

    let top = engine.top();
    let arbiter = Arbiter::new_and_register(
        &engine,
        &clock,
        top,
        "arb",
        num_sources,
        Box::new(RoundRobin::new()),
    )
    .unwrap();
    let sink_limiter =
        Limiter::new_and_register(&engine, &clock, top, "limit_sink", rate_limiter).unwrap();
    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();
    for i in 0..num_sources {
        sources.push(
            Source::new_and_register(
                &engine,
                top,
                &format!("source_{i}"),
                option_box_repeat!(i ; num_puts),
            )
            .unwrap(),
        );
        connect_port!(sources[i], tx => arbiter, rx, i).unwrap();
    }

    connect_port!(arbiter, tx => sink_limiter, rx).unwrap();
    connect_port!(sink_limiter, tx => sink, rx).unwrap();

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
