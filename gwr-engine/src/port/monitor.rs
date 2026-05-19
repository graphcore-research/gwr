// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

//! Monitor for port
//!
//! This port monitor is used to track data travelling through the
//! port and report bandwidth.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

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
    last_time: RefCell<Duration>,
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
            last_time: RefCell::new(clock.time_now()),
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

            let time_now = self.clock.time_now();
            let window_duration_s =
                (time_now.checked_sub(*self.last_time.borrow()).unwrap()).as_secs_f64();

            let per_second = Byte::from_f64(bytes_in_window as f64 / window_duration_s).unwrap();
            let gib_per_second = per_second.get_adjusted_unit(self.bw_unit);

            self.entity.track_value(gib_per_second.get_value());

            *self.last_time.borrow_mut() = time_now;
        }
    }
}
