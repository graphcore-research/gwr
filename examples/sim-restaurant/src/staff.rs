// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fmt;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::events::any_of::AnyOf;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::Event;

use crate::config::RestaurantConfig;
use crate::customer::CustomerOutcome;
use crate::menu::ORDERS;
use crate::sim::Restaurant;

#[derive(Clone, Copy, Debug)]
pub struct Staffing {
    pub till: usize,
    pub kitchen: usize,
}

impl fmt::Display for Staffing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "till {} / kitchen {}", self.till, self.kitchen)
    }
}

pub fn spawn_till_worker(
    engine: &Engine,
    clock: &Clock,
    config: RestaurantConfig,
    worker_id: usize,
    restaurant: Rc<Restaurant>,
) {
    let clock = clock.clone();
    engine.spawn(async move {
        loop {
            if clock.tick_now().tick() >= config.day_ticks {
                break;
            }

            if let Some(customer_id) = restaurant.pop_till() {
                let customer = restaurant.customer(customer_id);
                let till_start = restaurant.tick_now();
                customer.mark_till_started(till_start);
                restaurant.begin_till_service(
                    customer_id,
                    customer.joined_queue_tick(),
                    worker_id,
                    customer.order_index(),
                );

                let order_ticks = config.move_to_till_ticks
                    + config.order_overhead_ticks
                    + config.payment_ticks
                    + order_ordering_ticks(customer.order_index());
                clock.wait_ticks(order_ticks).await;

                let payment_tick = restaurant.tick_now();
                customer.mark_payment_done(payment_tick);
                restaurant.record_order_started(customer_id, worker_id);
                restaurant.enqueue_kitchen(customer_id).await?;
                restaurant.finish_till_service(customer_id, worker_id);
                continue;
            }

            if restaurant.arrivals_complete.get() && restaurant.till_queue.is_empty() {
                break;
            }

            AnyOf::new(vec![
                Box::new(restaurant.till_queue_changed()),
                Box::new(restaurant.closed.clone()),
            ])
            .listen()
            .await;
        }
        Ok(())
    });
}

pub fn spawn_kitchen_worker(
    engine: &Engine,
    clock: &Clock,
    config: RestaurantConfig,
    worker_id: usize,
    restaurant: Rc<Restaurant>,
) {
    let clock = clock.clone();
    engine.spawn(async move {
        loop {
            if let Some(customer_id) = restaurant.pop_kitchen() {
                let customer = restaurant.customer(customer_id);
                restaurant.begin_kitchen_service(customer_id, worker_id, customer.order_index());

                let prep_ticks = config.pack_order_ticks + order_prep_ticks(customer.order_index());
                clock.wait_ticks(prep_ticks).await;

                let ready_tick = restaurant.tick_now();
                customer.mark_food_ready(ready_tick);

                restaurant.record_order_served(
                    customer_id,
                    worker_id,
                    customer.payment_done_tick(),
                );

                customer.notify_outcome(CustomerOutcome::Served);
                restaurant.finish_kitchen_service(customer_id, worker_id);
                continue;
            }

            if restaurant.can_kitchen_exit() {
                break;
            }

            if restaurant.closed_seen.get() {
                restaurant.kitchen_queue_changed().listen().await;
            } else {
                AnyOf::new(vec![
                    Box::new(restaurant.kitchen_queue_changed()),
                    Box::new(restaurant.closed.clone()),
                ])
                .listen()
                .await;
            }
        }
        Ok(())
    });
}

fn order_ordering_ticks(order_index: usize) -> u64 {
    ORDERS[order_index]
        .items
        .iter()
        .map(|line| line.item.order_ticks() * line.count as u64)
        .sum()
}

fn order_prep_ticks(order_index: usize) -> u64 {
    ORDERS[order_index]
        .items
        .iter()
        .map(|line| line.item.prep_ticks() * line.count as u64)
        .sum()
}
