// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::ops::RangeInclusive;
use std::rc::Rc;

use gwr_components::queue::QueueCore;
use gwr_engine::engine::Engine;
use gwr_engine::events::once::Once;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::Event;
use gwr_engine::types::{SimError, SimResult};
use gwr_track::entity::Entity;
use gwr_track::tracker::dev_null_tracker;
use gwr_track::{Tracker, info, warn};

use crate::config::RestaurantConfig;
use crate::customer::{Customer, CustomerOutcome, CustomerPlan, generate_demand, spawn_arrivals};
use crate::menu::{order_name, order_value};
use crate::recording::{
    CustomerPhase, CustomerStateCounts, RecordedSimulation, SimulationSnapshot, TimelineEvent,
    TimelinePoint,
};
use crate::staff::{Staffing, spawn_kitchen_worker, spawn_till_worker};
use crate::time_of_day::TimeOfDay;

#[derive(Default, Debug, Clone)]
pub struct Metrics {
    pub arrivals: usize,
    pub balked: usize,
    pub gave_up_queue: usize,
    pub entered_queue: usize,
    pub till_started: usize,
    pub orders_started: usize,
    pub orders_served: usize,
    pub orders_abandoned: usize,
    pub revenue: f64,
    pub ingredient_cost: f64,
    pub till_wait_ticks_total: u64,
    pub kitchen_wait_ticks_total: u64,
    pub visit_ticks_total: u64,
    pub max_till_queue_len: usize,
    pub max_kitchen_queue_len: usize,
}

struct Recorder {
    timeline: RefCell<Vec<TimelinePoint>>,
    events: RefCell<Vec<TimelineEvent>>,
    customer_phases: RefCell<Vec<CustomerPhase>>,
}

impl Recorder {
    fn new(num_customers: usize) -> Self {
        Self {
            timeline: RefCell::new(Vec::new()),
            events: RefCell::new(Vec::new()),
            customer_phases: RefCell::new(vec![CustomerPhase::Planned; num_customers]),
        }
    }

    fn set_customer_phase(&self, customer_id: usize, phase: CustomerPhase) {
        if let Some(slot) = self.customer_phases.borrow_mut().get_mut(customer_id) {
            *slot = phase;
        }
    }

    fn snapshot_counts(&self) -> CustomerStateCounts {
        let mut counts = CustomerStateCounts::default();
        for phase in self.customer_phases.borrow().iter().copied() {
            match phase {
                CustomerPhase::Planned => counts.planned += 1,
                CustomerPhase::Balked => counts.balked += 1,
                CustomerPhase::WaitingTill => counts.waiting_till += 1,
                CustomerPhase::AtTill => counts.at_till += 1,
                CustomerPhase::WaitingKitchen => counts.waiting_kitchen += 1,
                CustomerPhase::PreparingFood => counts.preparing_food += 1,
                CustomerPhase::CollectingFood => counts.collecting_food += 1,
                CustomerPhase::Served => counts.served += 1,
                CustomerPhase::GaveUpQueue => counts.gave_up_queue += 1,
                CustomerPhase::Abandoned => counts.abandoned += 1,
            }
        }
        counts
    }

    fn record_snapshot(&self, tick: u64, restaurant: &Restaurant, message: String) {
        let snapshot = SimulationSnapshot {
            metrics: restaurant.metrics.borrow().clone(),
            till_queue: restaurant.till_queue.values(),
            kitchen_queue: restaurant.kitchen_queue.values(),
            active_till_workers: restaurant.active_till_workers.get(),
            active_kitchen_workers: restaurant.active_kitchen_workers.get(),
            arrivals_complete: restaurant.arrivals_complete.get(),
            closed: restaurant.closed_seen.get(),
            customer_counts: self.snapshot_counts(),
        };
        self.timeline
            .borrow_mut()
            .push(TimelinePoint { tick, snapshot });
        self.events
            .borrow_mut()
            .push(TimelineEvent { tick, message });
    }

    fn finish(
        &self,
        staffing: Staffing,
        opening_time: TimeOfDay,
        day_ticks: u64,
        summary: RunSummary,
        demand_size: usize,
    ) -> RecordedSimulation {
        RecordedSimulation {
            staffing,
            opening_time,
            day_ticks,
            summary,
            demand_size,
            timeline: self.timeline.borrow().clone(),
            events: self.events.borrow().clone(),
        }
    }
}

#[derive(Clone)]
struct ScenarioEntities {
    restaurant: Rc<Entity>,
    till_workers: Vec<Rc<Entity>>,
    kitchen_workers: Vec<Rc<Entity>>,
}

fn build_entities(engine: &Engine, staffing: Staffing) -> ScenarioEntities {
    let restaurant = Rc::new(Entity::new(engine.top(), "restaurant"));
    let staff = Rc::new(Entity::new(&restaurant, "staff"));
    let till_workers = (0..staffing.till)
        .map(|index| Rc::new(Entity::new(&staff, &format!("till_{index}"))))
        .collect();
    let kitchen_workers = (0..staffing.kitchen)
        .map(|index| Rc::new(Entity::new(&staff, &format!("kitchen_{index}"))))
        .collect();

    ScenarioEntities {
        restaurant,
        till_workers,
        kitchen_workers,
    }
}

pub(crate) struct Restaurant {
    clock: Clock,
    pub(crate) till_queue: QueueCore<usize>,
    pub(crate) kitchen_queue: QueueCore<usize>,
    pub(crate) closed: Once<()>,
    pub(crate) arrivals_complete: Cell<bool>,
    pub(crate) closed_seen: Cell<bool>,
    pub(crate) active_till_workers: Cell<usize>,
    pub(crate) active_kitchen_workers: Cell<usize>,
    pub(crate) metrics: RefCell<Metrics>,
    entity: Rc<Entity>,
    customers: RefCell<Vec<Option<Rc<Customer>>>>,
    till_worker_entities: Vec<Rc<Entity>>,
    kitchen_worker_entities: Vec<Rc<Entity>>,
    recorder: Option<Rc<Recorder>>,
}

impl Restaurant {
    #[must_use]
    fn new(
        clock: Clock,
        entities: ScenarioEntities,
        num_customers: usize,
        kitchen_queue_capacity: usize,
        recorder: Option<Rc<Recorder>>,
    ) -> Self {
        let entity = entities.restaurant;
        let till_queue =
            QueueCore::new(&entity, "till_queue", None).expect("queue config should be valid");
        let kitchen_queue = QueueCore::new(&entity, "kitchen_queue", Some(kitchen_queue_capacity))
            .expect("queue config should be valid");

        Self {
            clock,
            till_queue,
            kitchen_queue,
            closed: Once::default(),
            arrivals_complete: Cell::new(false),
            closed_seen: Cell::new(false),
            active_till_workers: Cell::new(0),
            active_kitchen_workers: Cell::new(0),
            metrics: RefCell::new(Metrics::default()),
            entity,
            customers: RefCell::new(vec![None; num_customers]),
            till_worker_entities: entities.till_workers,
            kitchen_worker_entities: entities.kitchen_workers,
            recorder,
        }
    }

    #[must_use]
    pub(crate) fn till_queue_changed(&self) -> Repeated<()> {
        self.till_queue.changed_event()
    }

    #[must_use]
    pub(crate) fn kitchen_queue_changed(&self) -> Repeated<()> {
        self.kitchen_queue.changed_event()
    }

    pub(crate) fn customer(&self, customer_id: usize) -> Rc<Customer> {
        self.customers.borrow()[customer_id]
            .clone()
            .expect("customer should be registered before use")
    }

    fn entity(&self) -> &Rc<Entity> {
        &self.entity
    }

    #[must_use]
    pub(crate) fn tick_now(&self) -> u64 {
        self.clock.tick_now().tick()
    }

    fn till_worker_entity(&self, worker_id: usize) -> &Rc<Entity> {
        &self.till_worker_entities[worker_id]
    }

    fn kitchen_worker_entity(&self, worker_id: usize) -> &Rc<Entity> {
        &self.kitchen_worker_entities[worker_id]
    }

    fn set_customer_phase(&self, customer_id: usize, phase: CustomerPhase) {
        if let Some(recorder) = &self.recorder {
            recorder.set_customer_phase(customer_id, phase);
        }
    }

    pub(crate) fn record_snapshot(&self, message: String) {
        if let Some(recorder) = &self.recorder {
            recorder.record_snapshot(self.tick_now(), self, message);
        }
    }

    pub(crate) fn customer_arrived(
        &self,
        customer: Rc<Customer>,
        queue_len: usize,
        join_probability: f64,
    ) -> usize {
        let customer_id = customer.id();
        self.customers.borrow_mut()[customer_id] = Some(customer);
        self.metrics.borrow_mut().arrivals += 1;

        let message = format!(
            "customer {customer_id} arrived, till queue len {queue_len}, join probability {join_probability:.2}",
        );
        self.record_snapshot(message.clone());
        info!(self.entity; "{message}");
        customer_id
    }

    pub(crate) fn customer_balked(&self, customer_id: usize) {
        self.set_customer_phase(customer_id, CustomerPhase::Balked);
        self.metrics.borrow_mut().balked += 1;

        let message = format!("customer {customer_id} balked at the queue");
        self.record_snapshot(message.clone());
        info!(self.entity(); "{message}");
    }

    pub(crate) fn customer_gave_up_in_queue(&self, customer_id: usize) {
        self.set_customer_phase(customer_id, CustomerPhase::GaveUpQueue);
        self.metrics.borrow_mut().gave_up_queue += 1;

        let message = format!("customer {customer_id} gave up waiting in the till queue");
        self.record_snapshot(message.clone());
        info!(self.entity(); "{message}");
    }

    pub(crate) fn customer_abandoned_after_order(&self, customer_id: usize) {
        self.set_customer_phase(customer_id, CustomerPhase::Abandoned);
        let customer = self.customer(customer_id);
        let refund = customer
            .payment_done_tick()
            .map(|_| order_value(customer.order_index()).0);
        let mut metrics = self.metrics.borrow_mut();
        metrics.orders_abandoned += 1;
        if let Some(refund) = refund {
            metrics.revenue -= refund;
        }
        drop(metrics);

        let message = if let Some(refund) = refund {
            format!("customer {customer_id} abandoned after ordering and was refunded {refund:.2}")
        } else {
            format!("customer {customer_id} abandoned after ordering")
        };
        self.record_snapshot(message.clone());
        warn!(self.entity(); "{message}");
    }

    pub(crate) fn customer_served(&self, customer_id: usize, joined_queue_tick: Option<u64>) {
        self.set_customer_phase(customer_id, CustomerPhase::Served);
        let tick = self.tick_now();
        if let Some(joined_tick) = joined_queue_tick {
            self.metrics.borrow_mut().visit_ticks_total += tick - joined_tick;
        }

        let message = format!("customer {customer_id} served");
        self.record_snapshot(message.clone());
        info!(self.entity(); "{message}");
    }

    #[must_use]
    pub(crate) fn queue_len(&self) -> usize {
        self.till_queue.len()
    }

    pub(crate) async fn enqueue_till(&self, customer_id: usize) -> SimResult {
        self.set_customer_phase(customer_id, CustomerPhase::WaitingTill);
        self.till_queue.push(customer_id).await?;
        let len = self.till_queue.len();

        let mut metrics = self.metrics.borrow_mut();
        metrics.entered_queue += 1;
        metrics.max_till_queue_len = metrics.max_till_queue_len.max(len);
        drop(metrics);

        let message = format!("customer {customer_id} joined the till queue");
        self.record_snapshot(message.clone());
        info!(self.entity(); "{message}");
        Ok(())
    }

    #[must_use]
    pub(crate) fn pop_till(&self) -> Option<usize> {
        self.till_queue.pop_front()
    }

    #[must_use]
    pub(crate) fn remove_till(&self, customer_id: usize) -> bool {
        self.till_queue
            .remove_where(|queued_id| *queued_id == customer_id)
            .is_some()
    }

    pub(crate) async fn enqueue_kitchen(&self, customer_id: usize) -> SimResult {
        self.set_customer_phase(customer_id, CustomerPhase::WaitingKitchen);
        while self.kitchen_queue.is_full() {
            self.kitchen_queue_changed().listen().await;
        }

        self.kitchen_queue.push(customer_id).await?;
        let len = self.kitchen_queue.len();

        let mut metrics = self.metrics.borrow_mut();
        metrics.max_kitchen_queue_len = metrics.max_kitchen_queue_len.max(len);
        drop(metrics);

        let message = format!("customer {customer_id} entered the kitchen queue");
        self.record_snapshot(message.clone());
        info!(self.entity(); "{message}");
        Ok(())
    }

    #[must_use]
    pub(crate) fn pop_kitchen(&self) -> Option<usize> {
        self.kitchen_queue.pop_front()
    }

    pub(crate) fn begin_till_service(
        &self,
        customer_id: usize,
        joined_tick: Option<u64>,
        worker_id: usize,
        order_index: usize,
    ) {
        let tick = self.tick_now();
        self.set_customer_phase(customer_id, CustomerPhase::AtTill);
        self.active_till_workers
            .set(self.active_till_workers.get() + 1);
        let mut metrics = self.metrics.borrow_mut();
        metrics.till_started += 1;
        if let Some(joined_tick) = joined_tick {
            metrics.till_wait_ticks_total += tick - joined_tick;
        }
        drop(metrics);

        let customer_message = format!("customer {customer_id} reached the till");
        info!(self.entity(); "{customer_message}");
        self.record_snapshot(customer_message);
        info!(
            self.till_worker_entity(worker_id);
            "started serving customer {customer_id} ({})",
            order_name(order_index)
        );
    }

    pub(crate) fn finish_till_service(&self, customer_id: usize, _worker_id: usize) {
        self.active_till_workers
            .set(self.active_till_workers.get().saturating_sub(1));
        self.record_snapshot(format!("till work completed for customer {customer_id}"));
    }

    pub(crate) fn record_order_started(&self, customer_id: usize, worker_id: usize) {
        let (revenue, _) = order_value(self.customer(customer_id).order_index());
        let mut metrics = self.metrics.borrow_mut();
        metrics.orders_started += 1;
        metrics.revenue += revenue;
        drop(metrics);

        let message = format!("customer {customer_id} finished paying {revenue:.2}");
        self.record_snapshot(message.clone());
        info!(self.till_worker_entity(worker_id); "{message}");
    }

    pub(crate) fn begin_kitchen_service(
        &self,
        customer_id: usize,
        worker_id: usize,
        order_index: usize,
    ) {
        self.set_customer_phase(customer_id, CustomerPhase::PreparingFood);
        self.active_kitchen_workers
            .set(self.active_kitchen_workers.get() + 1);
        let message = format!("kitchen started order for customer {customer_id}");
        self.record_snapshot(message.clone());
        info!(
            self.kitchen_worker_entity(worker_id);
            "started preparing customer {} ({})",
            customer_id,
            order_name(order_index)
        );
    }

    pub(crate) fn finish_kitchen_service(&self, customer_id: usize, _worker_id: usize) {
        self.active_kitchen_workers
            .set(self.active_kitchen_workers.get().saturating_sub(1));
        self.record_snapshot(format!(
            "kitchen finished service work for customer {customer_id}"
        ));
    }

    pub(crate) fn record_order_served(
        &self,
        customer_id: usize,
        worker_id: usize,
        payment_tick: Option<u64>,
    ) {
        let tick = self.tick_now();
        let (_, ingredient_cost) = order_value(self.customer(customer_id).order_index());
        let mut metrics = self.metrics.borrow_mut();
        metrics.orders_served += 1;
        metrics.ingredient_cost += ingredient_cost;
        if let Some(payment_tick) = payment_tick {
            metrics.kitchen_wait_ticks_total += tick - payment_tick;
        }
        drop(metrics);

        self.set_customer_phase(customer_id, CustomerPhase::CollectingFood);
        let message = format!("order ready for customer {customer_id}");
        self.record_snapshot(message.clone());
        info!(self.kitchen_worker_entity(worker_id); "{message}");
    }

    pub(crate) fn customer_collecting_food(&self, customer_id: usize) {
        let message = format!("order ready for customer {customer_id}");
        info!(self.entity(); "{message}");
    }

    pub(crate) fn mark_arrivals_complete(&self) -> SimResult {
        self.arrivals_complete.set(true);
        let message = "all arrivals processed".to_string();
        self.record_snapshot(message.clone());
        info!(self.entity; "{message}");
        Ok(())
    }

    pub(crate) fn mark_closed(&self) -> SimResult {
        self.closed_seen.set(true);
        let message = "restaurant closed".to_string();
        self.record_snapshot(message.clone());
        warn!(self.entity; "{message}");
        self.closed.notify()
    }

    fn record_removed_from_till_at_close(&self, customer_id: usize) {
        self.set_customer_phase(customer_id, CustomerPhase::Abandoned);
        let message = format!("customer {customer_id} removed from till queue at close");
        self.record_snapshot(message.clone());
        warn!(self.entity(); "{message}");
    }

    #[must_use]
    pub(crate) fn can_kitchen_exit(&self) -> bool {
        self.arrivals_complete.get()
            && self.till_queue.is_empty()
            && self.kitchen_queue.is_empty()
            && self.active_till_workers.get() == 0
    }
}

#[derive(Debug, Clone)]
pub struct RunSummary {
    pub staffing: Staffing,
    pub arrivals: usize,
    pub balked: usize,
    pub gave_up_queue: usize,
    pub served: usize,
    pub abandoned: usize,
    pub revenue: f64,
    pub ingredient_cost: f64,
    pub salary_cost: f64,
    pub profit: f64,
    pub avg_till_wait: f64,
    pub avg_kitchen_wait: f64,
    pub avg_visit: f64,
    pub max_till_queue_len: usize,
    pub max_kitchen_queue_len: usize,
    pub finish_tick: u64,
}

impl RunSummary {
    pub fn print_table_header() {
        println!(
            "{:>4} {:>7} {:>7} {:>8} {:>8} {:>10} {:>10} {:>9} {:>9} {:>10}",
            "Till",
            "Kitchen",
            "Served",
            "Balked",
            "GaveUp",
            "Revenue",
            "Costs",
            "Profit",
            "Finish h",
            "Max Queue"
        );
    }

    pub fn print_table_row(&self) {
        let costs = self.ingredient_cost + self.salary_cost;
        println!(
            "{:>4} {:>7} {:>7} {:>8} {:>8} {:>10.2} {:>10.2} {:>9.2} {:>9.2} {:>4}/{:<4}",
            self.staffing.till,
            self.staffing.kitchen,
            self.served,
            self.balked,
            self.gave_up_queue,
            self.revenue,
            costs,
            self.profit,
            self.finish_tick as f64 / 3600.0,
            self.max_till_queue_len,
            self.max_kitchen_queue_len
        );
    }

    pub fn print_best_summary(&self, day_ticks: u64) {
        println!("Best configuration: {}", self.staffing);
        println!(
            "Served {} of {} arrivals, balked {}, gave up in queue {}, abandoned after ordering {}.",
            self.served, self.arrivals, self.balked, self.gave_up_queue, self.abandoned
        );
        println!(
            "Revenue {:.2}, ingredient cost {:.2}, salary cost {:.2}, profit {:.2}.",
            self.revenue, self.ingredient_cost, self.salary_cost, self.profit
        );
        println!(
            "Average waits: till {:.1}s, kitchen {:.1}s, full visit {:.1}s.",
            self.avg_till_wait, self.avg_kitchen_wait, self.avg_visit
        );
        if self.finish_tick > day_ticks {
            println!(
                "Work completes {:.1} hours after opening, or {:.1} hours after close.",
                self.finish_tick as f64 / 3600.0,
                (self.finish_tick - day_ticks) as f64 / 3600.0
            );
        }
    }
}

pub struct ScenarioResult {
    pub summary: RunSummary,
    pub recording: Option<RecordedSimulation>,
}

pub fn run_sweep(
    config: &RestaurantConfig,
    till_range: RangeInclusive<usize>,
    kitchen_range: RangeInclusive<usize>,
    tracker: &Tracker,
) -> Result<(Vec<CustomerPlan>, Vec<RunSummary>), SimError> {
    let demand = generate_demand(config);
    let mut results = Vec::new();

    for till in till_range {
        for kitchen in kitchen_range.clone() {
            let staffing = Staffing { till, kitchen };
            results.push(run_configuration(config, &demand, staffing, false, tracker)?.summary);
        }
    }

    results.sort_by(|a, b| b.profit.total_cmp(&a.profit));
    Ok((demand, results))
}

pub fn run_recorded_scenario(
    config: &RestaurantConfig,
    staffing: Staffing,
) -> Result<RecordedSimulation, SimError> {
    let demand = generate_demand(config);
    let tracker = dev_null_tracker();
    let result = run_configuration(config, &demand, staffing, true, &tracker)?;
    result
        .recording
        .ok_or_else(|| SimError("expected recorded simulation".to_string()))
}

pub fn run_configuration(
    config: &RestaurantConfig,
    demand: &[CustomerPlan],
    staffing: Staffing,
    record_timeline: bool,
    tracker: &Tracker,
) -> Result<ScenarioResult, SimError> {
    let mut engine = Engine::new(tracker);

    let entities = build_entities(&engine, staffing);
    let clock = engine.clock_hz(1.0);
    let recorder = record_timeline.then(|| Rc::new(Recorder::new(demand.len())));
    let restaurant_entity = entities.restaurant.clone();
    let till_workers = entities.till_workers.clone();
    let kitchen_workers = entities.kitchen_workers.clone();
    let restaurant = Rc::new(Restaurant::new(
        clock.clone(),
        entities,
        demand.len(),
        config.max_kitchen_queue_len,
        recorder.clone(),
    ));
    let demand: Rc<Vec<CustomerPlan>> = Rc::new(demand.to_vec());

    if staffing.kitchen == 0 || staffing.till == 0 {
        return sim_error!("Invalid configuration with 0 staff on either till or in the kitchen");
    }

    restaurant.record_snapshot("simulation initialised".to_string());
    info!(
        restaurant_entity;
        "starting scenario with {} customer plans, till staff {}, kitchen staff {}",
        demand.len(),
        staffing.till,
        staffing.kitchen
    );

    spawn_arrivals(&engine, &clock, *config, restaurant.clone(), demand.clone());

    for (worker_id, _) in till_workers.iter().enumerate() {
        spawn_till_worker(&engine, &clock, *config, worker_id, restaurant.clone());
    }
    for (worker_id, _) in kitchen_workers.iter().enumerate() {
        spawn_kitchen_worker(&engine, &clock, *config, worker_id, restaurant.clone());
    }

    spawn_close_kitchen(&engine, &clock, config.day_ticks, restaurant.clone());

    engine.run()?;

    let metrics = restaurant.metrics.borrow().clone();
    let finish_tick = clock.tick_now().tick();
    let paid_hours = config.paid_hours() as f64;
    let salary_cost = paid_hours
        * (staffing.till as f64 * config.till_salary_per_hour
            + staffing.kitchen as f64 * config.kitchen_salary_per_hour);
    let profit = metrics.revenue - metrics.ingredient_cost - salary_cost;
    let served = metrics.orders_served.max(1) as f64;
    let till_started = metrics.till_started.max(1) as f64;

    let summary = RunSummary {
        staffing,
        arrivals: metrics.arrivals,
        balked: metrics.balked,
        gave_up_queue: metrics.gave_up_queue,
        served: metrics.orders_served,
        abandoned: metrics.orders_abandoned,
        revenue: metrics.revenue,
        ingredient_cost: metrics.ingredient_cost,
        salary_cost,
        profit,
        avg_till_wait: metrics.till_wait_ticks_total as f64 / till_started,
        avg_kitchen_wait: metrics.kitchen_wait_ticks_total as f64 / served,
        avg_visit: metrics.visit_ticks_total as f64 / served,
        max_till_queue_len: metrics.max_till_queue_len,
        max_kitchen_queue_len: metrics.max_kitchen_queue_len,
        finish_tick,
    };

    let recording = recorder.map(|recorder| {
        recorder.finish(
            staffing,
            config.opening_time,
            config.day_ticks,
            summary.clone(),
            demand.len(),
        )
    });
    Ok(ScenarioResult { summary, recording })
}

fn spawn_close_kitchen(engine: &Engine, clock: &Clock, day_ticks: u64, restaurant: Rc<Restaurant>) {
    let clock = clock.clone();
    engine.spawn(async move {
        clock.wait_ticks(day_ticks).await;
        restaurant.mark_closed()?;
        finish_till_queue_at_close(&restaurant);

        Ok(())
    });
}

fn finish_till_queue_at_close(restaurant: &Restaurant) {
    while let Some(customer_id) = restaurant.pop_till() {
        restaurant.record_removed_from_till_at_close(customer_id);
        restaurant
            .customer(customer_id)
            .notify_outcome(CustomerOutcome::KitchenClosed);
    }
}
