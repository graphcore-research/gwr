// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#[cfg_attr(not(target_os = "linux"), allow(unused))]
use gungraun::{library_benchmark, library_benchmark_group, main};

#[cfg_attr(not(target_os = "linux"), allow(unused))]
mod components;
use components::{
    run_engine, spawn_arbiter_fixedpriority_policy, spawn_larger_simulation,
    spawn_source_delay_sink, spawn_source_store_sink,
};
use gwr_engine::engine::Engine;

#[library_benchmark]
#[bench::source_store_sink(setup = spawn_source_store_sink)]
#[bench::source_delay_sink(setup = spawn_source_delay_sink)]
#[bench::arbiter_fixedpriority_policy(setup = spawn_arbiter_fixedpriority_policy)]
#[bench::larger_simulation(setup = spawn_larger_simulation)]
fn run_bench(engine: Engine) {
    run_engine(engine);
}

library_benchmark_group!(name = bench_group, benchmarks = run_bench);

cfg_if::cfg_if! {
    if #[cfg(target_os = "linux")] {
        main!(
            library_benchmark_groups = bench_group
        );
    } else {
        fn main() {
            println!("One-shot benchmarks are only supported on Linux");
        }
    }
}
