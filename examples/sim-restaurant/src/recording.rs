// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use crate::sim::{Metrics, RunSummary};
use crate::staff::Staffing;
use crate::time_of_day::TimeOfDay;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CustomerPhase {
    Planned,
    Balked,
    WaitingTill,
    AtTill,
    WaitingKitchen,
    PreparingFood,
    CollectingFood,
    Served,
    GaveUpQueue,
    Abandoned,
}

#[derive(Clone, Debug, Default)]
pub struct CustomerStateCounts {
    pub planned: usize,
    pub balked: usize,
    pub waiting_till: usize,
    pub at_till: usize,
    pub waiting_kitchen: usize,
    pub preparing_food: usize,
    pub collecting_food: usize,
    pub served: usize,
    pub gave_up_queue: usize,
    pub abandoned: usize,
}

#[derive(Clone, Debug)]
pub struct SimulationSnapshot {
    pub metrics: Metrics,
    pub till_queue: Vec<usize>,
    pub kitchen_queue: Vec<usize>,
    pub active_till_workers: usize,
    pub active_kitchen_workers: usize,
    pub arrivals_complete: bool,
    pub closed: bool,
    pub customer_counts: CustomerStateCounts,
}

#[derive(Clone, Debug)]
pub struct TimelinePoint {
    pub tick: u64,
    pub snapshot: SimulationSnapshot,
}

#[derive(Clone, Debug)]
pub struct TimelineEvent {
    pub tick: u64,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct RecordedSimulation {
    pub staffing: Staffing,
    pub opening_time: TimeOfDay,
    pub day_ticks: u64,
    pub summary: RunSummary,
    pub demand_size: usize,
    pub timeline: Vec<TimelinePoint>,
    pub events: Vec<TimelineEvent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlotStat {
    TillQueueLen,
    KitchenQueueLen,
    ActiveTillWorkers,
    ActiveKitchenWorkers,
    Arrivals,
    Balked,
    GaveUpQueue,
    OrdersStarted,
    OrdersServed,
    OrdersAbandoned,
    Revenue,
    IngredientCost,
    Profit,
}

impl PlotStat {
    pub const ALL: [Self; 13] = [
        Self::TillQueueLen,
        Self::KitchenQueueLen,
        Self::ActiveTillWorkers,
        Self::ActiveKitchenWorkers,
        Self::Arrivals,
        Self::Balked,
        Self::GaveUpQueue,
        Self::OrdersStarted,
        Self::OrdersServed,
        Self::OrdersAbandoned,
        Self::Revenue,
        Self::IngredientCost,
        Self::Profit,
    ];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::TillQueueLen => "Till Queue",
            Self::KitchenQueueLen => "Kitchen Queue",
            Self::ActiveTillWorkers => "Busy Till Workers",
            Self::ActiveKitchenWorkers => "Busy Kitchen Workers",
            Self::Arrivals => "Arrivals",
            Self::Balked => "Balked",
            Self::GaveUpQueue => "Gave Up Queue",
            Self::OrdersStarted => "Orders Started",
            Self::OrdersServed => "Orders Served",
            Self::OrdersAbandoned => "Orders Abandoned",
            Self::Revenue => "Revenue",
            Self::IngredientCost => "Ingredient Cost",
            Self::Profit => "Profit",
        }
    }

    #[must_use]
    pub fn supports_windowing(self) -> bool {
        matches!(
            self,
            Self::Arrivals
                | Self::Balked
                | Self::GaveUpQueue
                | Self::OrdersStarted
                | Self::OrdersServed
                | Self::OrdersAbandoned
                | Self::Revenue
                | Self::IngredientCost
                | Self::Profit
        )
    }

    #[must_use]
    pub fn title(self, windowed: bool, window_ticks: u64) -> String {
        if windowed && self.supports_windowing() {
            format!("{} (Windowed over {} ticks)", self.label(), window_ticks)
        } else {
            self.label().to_string()
        }
    }

    #[must_use]
    pub fn base_value(
        self,
        snapshot: &SimulationSnapshot,
        tick: u64,
        salary_cost: f64,
        day_ticks: u64,
    ) -> f64 {
        match self {
            Self::TillQueueLen => snapshot.till_queue.len() as f64,
            Self::KitchenQueueLen => snapshot.kitchen_queue.len() as f64,
            Self::ActiveTillWorkers => snapshot.active_till_workers as f64,
            Self::ActiveKitchenWorkers => snapshot.active_kitchen_workers as f64,
            Self::Arrivals => snapshot.metrics.arrivals as f64,
            Self::Balked => snapshot.metrics.balked as f64,
            Self::GaveUpQueue => snapshot.metrics.gave_up_queue as f64,
            Self::OrdersStarted => snapshot.metrics.orders_started as f64,
            Self::OrdersServed => snapshot.metrics.orders_served as f64,
            Self::OrdersAbandoned => snapshot.metrics.orders_abandoned as f64,
            Self::Revenue => snapshot.metrics.revenue,
            Self::IngredientCost => snapshot.metrics.ingredient_cost,
            Self::Profit => {
                let accrued_salary_cost = if day_ticks == 0 {
                    0.0
                } else {
                    salary_cost * (tick.min(day_ticks) as f64 / day_ticks as f64)
                };
                snapshot.metrics.revenue - snapshot.metrics.ingredient_cost - accrued_salary_cost
            }
        }
    }

    #[must_use]
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|stat| *stat == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    #[must_use]
    pub fn previous(self) -> Self {
        let index = Self::ALL.iter().position(|stat| *stat == self).unwrap_or(0);
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}
