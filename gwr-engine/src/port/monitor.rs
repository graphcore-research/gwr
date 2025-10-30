// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

//! Monitor for port
//!
//! This port monitor is used to track data travelling through the
//! port and report bandwidth.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use byte_unit::{Byte, Unit};
use gwr_track::entity::Entity;
use gwr_track::tracker::types::ReqType;
use gwr_track::{create, value};

use crate::engine::Engine;
use crate::time::clock::Clock;
use crate::traits::{Runnable, SimObject};
use crate::types::SimResult;

pub struct Monitor {
    pub bw_entity: Rc<Entity>,
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
        let bw_entity = Entity::new_without_create(entity, &format!("bw_{bw_unit}/s"));

        // Need to use custom create! in order to trackers know the data type.
        create!(entity ; bw_entity, 0, ReqType::Value as i8);

        let rc_self = Rc::new(Self {
            bw_entity: Rc::new(bw_entity),
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

            value!(self.bw_entity ; gib_per_second.get_value());

            *self.last_time_ns.borrow_mut() = time_now_ns;
        }
    }
}
