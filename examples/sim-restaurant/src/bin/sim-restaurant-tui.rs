// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Replay a single fast food restaurant scenario in a Ratatui TUI.
//!
//! See `sim-restaurant/src/lib.rs` for details.

use clap::Parser;
use gwr_engine::types::SimError;
use sim_restaurant::config::{RestaurantArgs, RestaurantConfig};
use sim_restaurant::sim::run_recorded_scenario;
use sim_restaurant::{Staffing, tui};

#[derive(Parser, Debug, Clone)]
#[command(about = "Replay a fast food restaurant scenario in a Ratatui TUI")]
struct TuiCli {
    /// Number of till workers in the replayed scenario.
    #[arg(long, default_value = "2")]
    till_staff: usize,

    /// Number of kitchen workers in the replayed scenario.
    #[arg(long, default_value = "4")]
    kitchen_staff: usize,

    #[command(flatten)]
    sim: RestaurantArgs,
}

impl TuiCli {
    fn validate(&self) -> Result<(), SimError> {
        RestaurantConfig::from(self.sim.clone()).validate()
    }

    fn sim_config(&self) -> RestaurantConfig {
        self.sim.clone().into()
    }

    fn staffing(&self) -> Staffing {
        Staffing {
            till: self.till_staff,
            kitchen: self.kitchen_staff,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = TuiCli::parse();
    cli.validate()?;

    let recording = run_recorded_scenario(&cli.sim_config(), cli.staffing())?;
    tui::run(recording)
}
