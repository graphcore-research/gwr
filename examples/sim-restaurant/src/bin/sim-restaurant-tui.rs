// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Replay a single fast food restaurant scenario in a Ratatui TUI.
//!
//! See `sim-restaurant/src/lib.rs` for details.

use gwr_config::multi_source_config;
use gwr_engine::types::SimError;
use sim_restaurant::config::{RestaurantArgs, RestaurantArgsPartial, RestaurantConfig};
use sim_restaurant::sim::run_recorded_scenario;
use sim_restaurant::{Staffing, tui};

#[multi_source_config(
    default_conf_file = "sim-restaurant-tui.toml",
    conf_file_flag = "conf-file"
)]
#[derive(Debug, Clone, PartialEq)]
#[command(about = "Replay a fast food restaurant scenario in a Ratatui TUI")]
struct TuiCli {
    /// Number of till workers in the replayed scenario.
    #[arg(long, default_value_t = 2)]
    till_staff: Option<usize>,

    /// Number of kitchen workers in the replayed scenario.
    #[arg(long, default_value_t = 4)]
    kitchen_staff: Option<usize>,

    #[command(flatten)]
    #[serde(flatten)]
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
            till: self.till_staff.unwrap(),
            kitchen: self.kitchen_staff.unwrap(),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = TuiCli::parse_all_sources();
    cli.validate()?;

    let recording = run_recorded_scenario(&cli.sim_config(), cli.staffing())?;
    tui::run(recording)
}
