// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::Cell;
use std::pin::pin;
use std::rc::Rc;

use futures::{FutureExt, select};
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::executor::Spawner;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::Event;
use gwr_engine::types::SimResult;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::config::RestaurantConfig;
use crate::menu::choose_order_index;
use crate::sim::Restaurant;
use crate::time_of_day::TimeOfDay;

#[derive(Clone, Debug)]
pub struct CustomerPlan {
    pub id: usize,
    pub arrival_tick: u64,
    pub order_index: usize,
    pub join_likelihood: f64,
}

#[derive(Clone, Copy, Debug)]
pub enum CustomerOutcome {
    Served,
    KitchenClosed,
}

pub struct Customer {
    plan: CustomerPlan,
    outcome: Repeated<CustomerOutcome>,
    joined_queue_tick: Cell<Option<u64>>,
    till_start_tick: Cell<Option<u64>>,
    payment_done_tick: Cell<Option<u64>>,
    food_ready_tick: Cell<Option<u64>>,
    departed_tick: Cell<Option<u64>>,
}

impl Customer {
    pub fn new(plan: CustomerPlan) -> Self {
        Self {
            plan,
            outcome: Repeated::new(CustomerOutcome::KitchenClosed),
            joined_queue_tick: Cell::new(None),
            till_start_tick: Cell::new(None),
            payment_done_tick: Cell::new(None),
            food_ready_tick: Cell::new(None),
            departed_tick: Cell::new(None),
        }
    }

    #[must_use]
    pub fn id(&self) -> usize {
        self.plan.id
    }

    #[must_use]
    pub fn order_index(&self) -> usize {
        self.plan.order_index
    }

    #[must_use]
    pub fn join_likelihood(&self) -> f64 {
        self.plan.join_likelihood
    }

    #[must_use]
    pub fn arrival_tick(&self) -> u64 {
        self.plan.arrival_tick
    }

    #[must_use]
    pub fn joined_queue_tick(&self) -> Option<u64> {
        self.joined_queue_tick.get()
    }

    #[must_use]
    pub fn payment_done_tick(&self) -> Option<u64> {
        self.payment_done_tick.get()
    }

    pub fn mark_joined_queue(&self, tick: u64) {
        self.joined_queue_tick.set(Some(tick));
    }

    pub fn mark_till_started(&self, tick: u64) {
        self.till_start_tick.set(Some(tick));
    }

    pub fn mark_payment_done(&self, tick: u64) {
        self.payment_done_tick.set(Some(tick));
    }

    pub fn mark_food_ready(&self, tick: u64) {
        self.food_ready_tick.set(Some(tick));
    }

    pub fn notify_outcome(&self, outcome: CustomerOutcome) {
        self.outcome.notify_result(outcome);
    }

    pub fn spawn(
        self: &Rc<Self>,
        spawner: &Spawner,
        config: RestaurantConfig,
        clock: &Clock,
        restaurant: &Rc<Restaurant>,
    ) {
        let spawner = spawner.clone();
        let customer = self.clone();
        let clock = clock.clone();
        let restaurant = restaurant.clone();
        let run_spawner = spawner.clone();
        spawner.spawn(async move { customer.run(run_spawner, config, clock, restaurant).await });
    }

    async fn run(
        self: Rc<Self>,
        _spawner: Spawner,
        config: RestaurantConfig,
        clock: Clock,
        restaurant: Rc<Restaurant>,
    ) -> SimResult {
        let mut outcome = pin!(self.outcome.listen().fuse());
        let mut timeout = pin!(clock.wait_ticks(config.max_queue_wait_ticks).fuse());

        select! {
            outcome = outcome => {
                self.handle_outcome(outcome, config, &clock, &restaurant).await;
            }
            _ = timeout => {
                if self.till_start_tick.get().is_none()
                    && self.departed_tick.get().is_none()
                && restaurant.remove_till(self.id())
                {
                    self.departed_tick.set(Some(restaurant.tick_now()));
                    restaurant.customer_gave_up_in_queue(self.id());
                } else {
                    self.handle_outcome(outcome.await, config, &clock, &restaurant)
                        .await;
                }
            }
        }

        Ok(())
    }

    async fn handle_outcome(
        &self,
        outcome: CustomerOutcome,
        config: RestaurantConfig,
        clock: &Clock,
        restaurant: &Restaurant,
    ) {
        match outcome {
            CustomerOutcome::Served => {
                restaurant.customer_collecting_food(self.id());
                clock.wait_ticks(config.take_food_ticks).await;
                clock.wait_ticks(config.leave_ticks).await;
                self.departed_tick.set(Some(restaurant.tick_now()));
                restaurant.customer_served(self.id(), self.joined_queue_tick.get());
            }
            CustomerOutcome::KitchenClosed => {
                restaurant.customer_abandoned_after_order(self.id());
            }
        }
    }
}

#[must_use]
pub fn generate_demand(config: &RestaurantConfig) -> Vec<CustomerPlan> {
    let mut rng = StdRng::seed_from_u64(config.seed);
    let mut customers = Vec::new();
    let mut tick = 0_u64;

    while tick < config.day_ticks {
        let multiplier = demand_multiplier(config, tick);
        let base_gap = (config.base_arrival_gap as f64 / multiplier).round() as i64;
        let jitter = rng.random_range(-config.arrival_jitter..=config.arrival_jitter);
        let gap = (base_gap + jitter).max(1) as u64;
        tick += gap;
        if tick >= config.day_ticks {
            break;
        }

        customers.push(CustomerPlan {
            id: customers.len(),
            arrival_tick: tick,
            order_index: choose_order_index(&mut rng),
            join_likelihood: rng.random::<f64>(),
        });
    }

    customers
}

pub fn spawn_arrivals(
    engine: &Engine,
    clock: &Clock,
    config: RestaurantConfig,
    restaurant: Rc<Restaurant>,
    demand: Rc<Vec<CustomerPlan>>,
) {
    let spawner = engine.spawner();
    let clock = clock.clone();
    engine.spawn(async move {
        let mut last_tick = 0_u64;
        for customer_plan in demand.iter() {
            clock
                .wait_ticks(customer_plan.arrival_tick - last_tick)
                .await;
            last_tick = customer_plan.arrival_tick;

            let customer = Rc::new(Customer::new(customer_plan.clone()));
            let queue_len = restaurant.queue_len();
            let join_probability = queue_join_probability(&config, queue_len);
            let customer_id =
                restaurant.customer_arrived(customer.clone(), queue_len, join_probability);
            if customer.join_likelihood() > join_probability {
                restaurant.customer_balked(customer_id);
                continue;
            }

            customer.mark_joined_queue(customer.arrival_tick());
            restaurant.enqueue_till(customer_id).await?;
            customer.spawn(&spawner, config, &clock, &restaurant);
        }

        restaurant.mark_arrivals_complete()?;
        Ok(())
    });
}

fn queue_join_probability(config: &RestaurantConfig, queue_len: usize) -> f64 {
    (config.join_base_probability * (-config.join_queue_sensitivity * queue_len as f64).exp())
        .clamp(0.0, 1.0)
}

fn demand_multiplier(config: &RestaurantConfig, tick: u64) -> f64 {
    let time = config.opening_time.add_ticks(tick);
    if time_in_range(time, TimeOfDay::from_hm(7, 0), TimeOfDay::from_hm(9, 30)) {
        2.45
    } else if time_in_range(time, TimeOfDay::from_hm(12, 0), TimeOfDay::from_hm(14, 30)) {
        5.35
    } else if time_in_range(time, TimeOfDay::from_hm(18, 0), TimeOfDay::from_hm(20, 30)) {
        3.25
    } else {
        1.0
    }
}

fn time_in_range(time: TimeOfDay, start: TimeOfDay, end: TimeOfDay) -> bool {
    start <= time && time < end
}
