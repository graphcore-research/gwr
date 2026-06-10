// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

//! Monitor for port
//!
//! This port monitor is used to track data travelling through the
//! port and report bandwidth.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use byte_unit::{Byte, Unit};
use gwr_track::entity::{Entity, EntityMonitor};

use crate::engine::Engine;
use crate::time::clock::Clock;
use crate::traits::{Runnable, SimObject};
use crate::types::SimResult;

pub struct Monitor {
    entity: EntityMonitor,
    clock: Clock,
    window_size_ticks: u64,
    bytes_in_window: RefCell<usize>,
    bytes_total: RefCell<usize>,
    last_time_ns: RefCell<f64>,
    bw_unit: Unit,
}

impl Monitor {
    #[must_use]
    pub fn new_and_register(
        engine: &Engine,
        entity: &Rc<Entity>,
        clock: &Clock,
        window_size_ticks: u64,
    ) -> Rc<Self> {
        let bw_unit = Unit::GiB;
        let bw_entity = EntityMonitor::new(entity, &format!("bw_{bw_unit}/s"));

        let rc_self = Rc::new(Self {
            entity: bw_entity,
            clock: clock.clone(),
            window_size_ticks,
            bytes_in_window: RefCell::new(0),
            bytes_total: RefCell::new(0),
            last_time_ns: RefCell::new(clock.time_now_ns()),
            bw_unit,
        });

        engine.register(rc_self.clone());
        rc_self
    }

    pub fn sample<T>(&self, object: &T)
    where
        T: SimObject,
    {
        let object_bytes = object.total_bytes();
        *self.bytes_in_window.borrow_mut() += object_bytes;
    }

    #[cfg(test)]
    pub(crate) fn bytes_in_window(&self) -> usize {
        *self.bytes_in_window.borrow()
    }

    #[cfg(test)]
    pub(crate) fn bytes_total(&self) -> usize {
        *self.bytes_total.borrow()
    }

    #[cfg(test)]
    pub(crate) fn last_time_ns(&self) -> f64 {
        *self.last_time_ns.borrow()
    }
}

#[async_trait(?Send)]
impl Runnable for Monitor {
    async fn run(&self) -> SimResult {
        // Drive the output
        loop {
            self.clock.wait_ticks_or_exit(self.window_size_ticks).await;
            let bytes_in_window = *self.bytes_in_window.borrow();
            *self.bytes_in_window.borrow_mut() = 0;
            *self.bytes_total.borrow_mut() += bytes_in_window;

            let time_now_ns = self.clock.time_now_ns();
            let window_duration_s =
                (time_now_ns - *self.last_time_ns.borrow()) / (1000.0 * 1000.0 * 1000.0);

            let per_second = Byte::from_f64(bytes_in_window as f64 / window_duration_s).unwrap();
            let gib_per_second = per_second.get_adjusted_unit(self.bw_unit);

            self.entity.track_value(gib_per_second.get_value());

            *self.last_time_ns.borrow_mut() = time_now_ns;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;
    use std::rc::Rc;

    use gwr_track::entity::Entity;
    use gwr_track::tracker::dev_null_tracker;

    use super::*;

    #[test]
    fn new_and_register_initializes_monitor_and_sample_counts_bytes() {
        let tracker = dev_null_tracker();
        let mut engine = Engine::new(&tracker);
        let clock = engine.default_clock();
        let parent = engine.top().clone();
        let entity = Rc::new(Entity::new(&parent, "port"));

        let monitor = Monitor::new_and_register(&engine, &entity, &clock, 4);

        assert_eq!(monitor.window_size_ticks, 4);
        assert_eq!(monitor.bytes_in_window(), 0);
        assert_eq!(monitor.bytes_total(), 0);
        assert_eq!(monitor.last_time_ns(), 0.0);

        monitor.sample(&123_i32);

        assert_eq!(monitor.bytes_in_window(), size_of::<i32>());

        {
            let clock = clock.clone();
            engine.spawn(async move {
                clock.wait_ticks(4).await;
                Ok(())
            });
        }

        engine.run().unwrap();

        assert_eq!(monitor.bytes_in_window(), 0);
        assert_eq!(monitor.bytes_total(), size_of::<i32>());
    }
}
