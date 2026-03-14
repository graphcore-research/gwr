// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::time::Duration;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

mod ethernet_frame;
use crate::ethernet_frame::{run_engine, setup_box_frame_simulation, setup_frame_simulation};

fn bench_ethernet_frame(c: &mut Criterion) {
    let mut group = c.benchmark_group("ethernet_frame");

    group.bench_function("vec_of_frame", |b| {
        b.iter_batched(setup_frame_simulation, run_engine, BatchSize::SmallInput);
    });

    group.bench_function("vec_of_box", |b| {
        b.iter_batched(
            setup_box_frame_simulation,
            run_engine,
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(6))
        .noise_threshold(0.03)
        .confidence_level(0.98);
    targets = bench_ethernet_frame
}

criterion_main! {
    benches,
}
