// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Simulate a fast food restaurant.
//!
//! See `lib.rs` for details.

use clap::CommandFactory;
use gwr_config::multi_source_config;
use gwr_engine::sim_error;
use gwr_engine::types::SimError;
use gwr_track::builder::{TrackerArgs, TrackerArgsPartial, setup_trackers};
use gwr_track::tracker::dev_null_tracker;
use sim_restaurant::config::{
    RestaurantArgs, RestaurantArgsPartial, RestaurantConfig, long_arg_name,
};
use sim_restaurant::sim::{RunSummary, run_sweep};

const DEFAULT_MIN_TILL_STAFF: usize = 1;
const DEFAULT_MAX_TILL_STAFF: usize = 4;
const DEFAULT_MIN_KITCHEN_STAFF: usize = 1;
const DEFAULT_MAX_KITCHEN_STAFF: usize = 5;
const DEFAULT_TOP_RESULTS: usize = 8;

#[multi_source_config(
    default_conf_file = "sim-restaurant.toml",
    conf_file_flag = "conf-file"
)]
#[derive(Debug, Clone, PartialEq)]
#[command(about = "Fast food restaurant profitability simulation")]
pub struct CliArgs {
    /// Minimum number of till workers to evaluate.
    #[arg(long, default_value_t = DEFAULT_MIN_TILL_STAFF)]
    pub min_till_staff: usize,

    /// Maximum number of till workers to evaluate.
    #[arg(long, default_value_t = DEFAULT_MAX_TILL_STAFF)]
    pub max_till_staff: usize,

    /// Minimum number of kitchen workers to evaluate.
    #[arg(long, default_value_t = DEFAULT_MIN_KITCHEN_STAFF)]
    pub min_kitchen_staff: usize,

    /// Maximum number of kitchen workers to evaluate.
    #[arg(long, default_value_t = DEFAULT_MAX_KITCHEN_STAFF)]
    pub max_kitchen_staff: usize,

    /// How many top staffing combinations to print.
    #[arg(long, default_value_t = DEFAULT_TOP_RESULTS)]
    pub top_results: usize,

    #[command(flatten)]
    #[serde(flatten)]
    pub tracker: TrackerArgs,

    #[command(flatten)]
    #[serde(flatten)]
    pub sim: RestaurantArgs,
}

impl CliArgs {
    pub fn validate(&self) -> Result<(), SimError> {
        let command = Self::command();
        let min_till_staff = long_arg_name(&command, "min_till_staff");
        let max_till_staff = long_arg_name(&command, "max_till_staff");
        let min_kitchen_staff = long_arg_name(&command, "min_kitchen_staff");
        let max_kitchen_staff = long_arg_name(&command, "max_kitchen_staff");

        if self.min_till_staff > self.max_till_staff {
            return sim_error!("`{min_till_staff}` must be <= `{max_till_staff}`");
        }
        if self.min_kitchen_staff > self.max_kitchen_staff {
            return sim_error!("`{min_kitchen_staff}` must be <= `{max_kitchen_staff}`");
        }
        if self.tracking_requested()
            && (self.min_till_staff != self.max_till_staff
                || self.min_kitchen_staff != self.max_kitchen_staff)
        {
            return sim_error!(
                "tracking output requires exactly one staffing configuration; set `{min_till_staff}` equal to `{max_till_staff}` and `{min_kitchen_staff}` equal to `{max_kitchen_staff}`"
            );
        }
        RestaurantConfig::from(self.sim.clone()).validate()
    }

    #[must_use]
    pub fn sim_config(&self) -> RestaurantConfig {
        self.sim.clone().into()
    }

    #[must_use]
    pub fn tracking_requested(&self) -> bool {
        self.tracker.tracking_requested()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = CliArgs::parse_all_sources();
    cli.validate()?;

    let config = cli.sim_config();

    let tracker = if cli.tracking_requested() {
        setup_trackers(&cli.tracker.trackers_config())
            .map_err(|err| std::io::Error::other(format!("{err:?}")))?
    } else {
        dev_null_tracker()
    };

    let (demand, results) = run_sweep(
        &config,
        cli.min_till_staff..=cli.max_till_staff,
        cli.min_kitchen_staff..=cli.max_kitchen_staff,
        &tracker,
    )?;

    println!(
        "Restaurant demand plan: {} customers from {} to {} ({:.1} hours, seed {}).",
        demand.len(),
        config.opening_time,
        config.closing_time,
        config.day_ticks as f64 / 3600.0,
        config.seed
    );

    println!();
    RunSummary::print_table_header();
    for summary in results.iter().take(cli.top_results) {
        summary.print_table_row();
    }

    if let Some(best) = results.first() {
        println!();
        best.print_best_summary(config.day_ticks);
    }

    Ok(())
}
