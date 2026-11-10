// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_platform::types::PlatformConfig;
use gwr_platform::yaml::platform_to_yaml_str;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;
use gwr_track::tracker::dev_null_tracker;

mod gemm_add;
mod platform;

#[derive(Clone, Copy)]
pub enum Workload {
    /// Independent chains of blocks computing `Y = X * W_left + X * W_right`:
    /// two matmuls from the same input, followed by an elementwise addition.
    /// Each block's output `Y` becomes the next block's input `X`.
    ///
    /// The 60 PEs form 30 pairs, alternating between two chain sizes:
    /// - 15 pairs run four blocks with FP32 32x32 inputs and weights. Each
    ///   operation fits in one PE's SRAM, and the chains reuse cached data.
    /// - 15 pairs run two blocks with FP32 128x64 inputs and 64x64 weights.
    ///   Each operation is computed in two serial SRAM partitions by its PE.
    ///
    /// Each pair has a different input matrix and reuses one assigned pair of
    /// weights throughout its chain. Each matrix size has its own shared pool
    /// of four weight matrices. One PE always computes the left matmul and the
    /// other the right; the addition runs on one PE, alternating between
    /// blocks. All outputs occupy fresh memory regions, with no output buffer
    /// reuse.
    ///
    /// Private L1 caches are coherent: shared weights exercise shared reads,
    /// and outputs passed between PEs exercise invalidations and dirty
    /// writebacks through a directory for each HBM bank.
    GemmAdd,
}

impl Workload {
    /// All cases available to the benchmark harnesses.
    #[allow(dead_code)] // Gungraun registers cases explicitly instead of iterating.
    pub const ALL: [Self; 1] = [Self::GemmAdd];

    pub fn name(self) -> &'static str {
        match self {
            Self::GemmAdd => "gemm_add",
        }
    }

    pub fn setup(self) -> Simulation {
        match self {
            Self::GemmAdd => gemm_add::setup(self.name()),
        }
    }
}

pub struct Simulation {
    pub engine: Engine,
    pub platform: Rc<Platform>,
    pub timetable: Rc<Timetable>,
    name: &'static str,
    validate_workload: fn(&Self),
}

impl Simulation {
    pub(super) fn new(
        name: &'static str,
        platform_config: &PlatformConfig,
        timetable_file: TimetableFile,
        validate_workload: fn(&Self),
    ) -> Self {
        let mut engine = Engine::new(&dev_null_tracker());
        let clock = engine.default_clock();
        let yaml = platform_to_yaml_str(platform_config).expect("serialize benchmark platform");
        let platform = Rc::new(
            Platform::from_string(&engine, &clock, &yaml).expect("construct benchmark platform"),
        );
        let graph = timetable_file
            .into_graph()
            .expect("construct benchmark graph");
        let timetable = Rc::new(
            Timetable::new(engine.top(), graph, &platform).expect("construct benchmark timetable"),
        );
        // Match ordinary application ownership.
        let dispatcher: Rc<dyn Dispatch> = timetable.clone();
        platform.attach_dispatcher(&dispatcher);
        Self {
            engine,
            platform,
            timetable,
            name,
            validate_workload,
        }
    }

    pub fn run(&mut self) {
        self.engine.run().expect("benchmark simulation failed");
    }

    pub fn validate(&self) {
        self.timetable
            .check_tasks_complete()
            .unwrap_or_else(|error| panic!("{}: {error}", self.name));
        assert!(
            self.engine.time_now_ns() > 0.0,
            "{}: no simulated time",
            self.name
        );
        (self.validate_workload)(self);
    }
}
