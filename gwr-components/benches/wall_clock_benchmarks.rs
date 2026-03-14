// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::time::Duration;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

mod components;
use crate::components::{
    run_engine, spawn_arbiter_fixedpriority_policy, spawn_larger_simulation,
    spawn_source_delay_sink, spawn_source_store_sink,
};

fn bench_small_blocks(c: &mut Criterion) {
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

    group.finish();
}

fn bench_large_blocks(c: &mut Criterion) {
    let mut group = c.benchmark_group("blocks");

    group.bench_function("larger_simulation", |b| {
        b.iter_batched(spawn_larger_simulation, run_engine, BatchSize::SmallInput);
    });

    group.finish();
}

criterion_group! {
    name = small_benches;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(6))
        .noise_threshold(0.05)
        .confidence_level(0.98)
        .significance_level(0.03);
    targets = bench_small_blocks
}

criterion_group! {
    name = large_benches;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .noise_threshold(0.02)
        .confidence_level(0.98);
    targets = bench_large_blocks
}

criterion_main! {
    small_benches, large_benches
}
