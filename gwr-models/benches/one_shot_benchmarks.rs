// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

#[cfg_attr(not(target_os = "linux"), allow(unused))]
use gungraun::{library_benchmark, library_benchmark_group, main};

#[cfg_attr(not(target_os = "linux"), allow(unused))]
mod ethernet_frame;
use ethernet_frame::{run_engine, setup_box_frame_simulation, setup_frame_simulation};
use gwr_components::sink::Sink;
use gwr_engine::engine::Engine;
use gwr_engine::traits::SimObject;

#[library_benchmark]
#[bench::frame_simulation(setup = setup_frame_simulation)]
#[bench::box_frame_simulation(setup = setup_box_frame_simulation)]
fn run_bench<T>(args: (Engine, Rc<Sink<T>>))
where
    T: SimObject,
{
    run_engine(args);
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
