// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#[cfg_attr(not(target_os = "linux"), allow(unused))]
use gungraun::{library_benchmark, library_benchmark_group, main};

#[cfg_attr(not(target_os = "linux"), allow(unused))]
mod timetable;
use timetable::{Simulation, Workload};

#[library_benchmark]
#[bench::gemm_add(args = (Workload::GemmAdd), setup = Workload::setup, teardown = teardown)]
fn run_bench(mut simulation: Simulation) -> Simulation {
    simulation.run();
    simulation
}

fn teardown(simulation: Simulation) {
    simulation.validate();
    drop(simulation);
}

library_benchmark_group!(name = bench_group, benchmarks = run_bench);

cfg_select! {
    target_os = "linux" => {
        main!(library_benchmark_groups = bench_group);
    }
    _ => {
        fn main() {
            println!("One-shot benchmarks are only supported on Linux");
        }
    }
}
