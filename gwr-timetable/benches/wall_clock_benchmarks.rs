// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::time::{Duration, Instant};

use criterion::{Criterion, SamplingMode, criterion_group, criterion_main};

mod timetable;
use timetable::Workload;

fn bench_timetables(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("timetable");
    group
        .sample_size(10)
        .sampling_mode(SamplingMode::Flat)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));

    for workload in Workload::ALL {
        group.bench_function(workload.name(), |bencher| {
            bencher.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    let mut simulation = workload.setup();
                    let start = Instant::now();
                    simulation.run();
                    elapsed += start.elapsed();
                    simulation.validate();
                }
                elapsed
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_timetables);
criterion_main!(benches);
