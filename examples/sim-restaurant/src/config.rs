// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use clap::{Args, Command};
use gwr_engine::types::SimError;

use crate::time_of_day::TimeOfDay;

#[derive(Debug, Clone, Args)]
pub struct RestaurantArgs {
    /// Random seed used to build the day's customer demand.
    #[arg(long, default_value = "7")]
    pub seed: u64,

    /// Restaurant opening time in 24-hour format.
    #[arg(long, default_value = "07:00")]
    pub opening_time: TimeOfDay,

    /// Restaurant closing time in 24-hour format.
    #[arg(long, default_value = "22:00")]
    pub closing_time: TimeOfDay,

    /// Average gap between arrivals during a normal period (in seconds).
    #[arg(long, default_value = "46")]
    pub base_arrival_gap: u64,

    /// Random jitter applied to each arrival gap (in seconds).
    #[arg(long, default_value = "14")]
    pub arrival_jitter: i64,

    /// Maximum chance that a customer joins when there is no queue.
    #[arg(long, default_value = "0.98")]
    pub join_base_probability: f64,

    /// Exponential drop-off in queue joining probability per person in line.
    #[arg(long, default_value = "0.17")]
    pub join_queue_sensitivity: f64,

    /// Maximum time a customer will stay in the till queue before leaving (in
    /// seconds).
    #[arg(long, default_value = "240")]
    pub max_queue_wait_ticks: u64,

    /// Time for a customer to walk from the queue to the till (in seconds).
    #[arg(long, default_value = "6")]
    pub move_to_till_ticks: u64,

    /// Fixed ordering overhead on top of item-specific times (in seconds).
    #[arg(long, default_value = "7")]
    pub order_overhead_ticks: u64,

    /// Time spent paying at the till (in seconds).
    #[arg(long, default_value = "15")]
    pub payment_ticks: u64,

    /// Time kitchen staff spend packing order (in seconds).
    #[arg(long, default_value = "12")]
    pub pack_order_ticks: u64,

    /// Maximum number of orders allowed to be queued waiting for the kitchen.
    #[arg(long, default_value = "24")]
    pub max_kitchen_queue_len: usize,

    /// Time for the customer to collect their food (in seconds).
    #[arg(long, default_value = "7")]
    pub take_food_ticks: u64,

    /// Time for the customer to leave after collecting food (in seconds).
    #[arg(long, default_value = "10")]
    pub leave_ticks: u64,

    /// Hourly pay cost of one till worker.
    #[arg(long, default_value = "16.0")]
    pub till_salary_per_hour: f64,

    /// Hourly pay cost of one kitchen worker.
    #[arg(long, default_value = "18.0")]
    pub kitchen_salary_per_hour: f64,
}

#[must_use]
pub fn long_arg_name(command: &Command, id: &str) -> String {
    let long = command
        .get_arguments()
        .find(|arg| arg.get_id().as_str() == id)
        .and_then(|arg| arg.get_long())
        .map_or_else(|| id.replace('_', "-"), ToOwned::to_owned);
    format!("--{long}")
}

#[derive(Debug, Clone, Copy)]
pub struct RestaurantConfig {
    pub seed: u64,
    pub opening_time: TimeOfDay,
    pub closing_time: TimeOfDay,
    pub day_ticks: u64,
    pub base_arrival_gap: u64,
    pub arrival_jitter: i64,
    pub join_base_probability: f64,
    pub join_queue_sensitivity: f64,
    pub max_queue_wait_ticks: u64,
    pub move_to_till_ticks: u64,
    pub order_overhead_ticks: u64,
    pub payment_ticks: u64,
    pub pack_order_ticks: u64,
    pub max_kitchen_queue_len: usize,
    pub take_food_ticks: u64,
    pub leave_ticks: u64,
    pub till_salary_per_hour: f64,
    pub kitchen_salary_per_hour: f64,
}

impl From<RestaurantArgs> for RestaurantConfig {
    fn from(restaurant_args: RestaurantArgs) -> Self {
        let day_ticks = restaurant_args
            .closing_time
            .seconds_since_midnight()
            .saturating_sub(restaurant_args.opening_time.seconds_since_midnight());
        Self {
            seed: restaurant_args.seed,
            opening_time: restaurant_args.opening_time,
            closing_time: restaurant_args.closing_time,
            day_ticks,
            base_arrival_gap: restaurant_args.base_arrival_gap,
            arrival_jitter: restaurant_args.arrival_jitter,
            join_base_probability: restaurant_args.join_base_probability,
            join_queue_sensitivity: restaurant_args.join_queue_sensitivity,
            max_queue_wait_ticks: restaurant_args.max_queue_wait_ticks,
            move_to_till_ticks: restaurant_args.move_to_till_ticks,
            order_overhead_ticks: restaurant_args.order_overhead_ticks,
            payment_ticks: restaurant_args.payment_ticks,
            pack_order_ticks: restaurant_args.pack_order_ticks,
            max_kitchen_queue_len: restaurant_args.max_kitchen_queue_len,
            take_food_ticks: restaurant_args.take_food_ticks,
            leave_ticks: restaurant_args.leave_ticks,
            till_salary_per_hour: restaurant_args.till_salary_per_hour,
            kitchen_salary_per_hour: restaurant_args.kitchen_salary_per_hour,
        }
    }
}

impl RestaurantConfig {
    pub fn validate(&self) -> Result<(), SimError> {
        let command = RestaurantArgs::augment_args(Command::new("sim-restaurant"));
        let join_base_probability = long_arg_name(&command, "join_base_probability");
        let opening_time = long_arg_name(&command, "opening_time");
        let closing_time = long_arg_name(&command, "closing_time");

        if !(0.0..=1.0).contains(&self.join_base_probability) {
            return Err(SimError(format!(
                "`{join_base_probability}` must be in the range 0..=1"
            )));
        }
        if self.opening_time >= self.closing_time {
            return Err(SimError(format!(
                "`{opening_time}` must be earlier than `{closing_time}`"
            )));
        }
        if self.day_ticks == 0 {
            return Err(SimError("day length must be greater than zero".to_string()));
        }
        Ok(())
    }

    #[must_use]
    pub fn paid_hours(&self) -> u64 {
        self.day_ticks.div_ceil(3600)
    }
}
