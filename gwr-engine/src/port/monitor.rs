// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

//! Monitor for port
//!
//! This port monitor is used to track data travelling through the
//! port and report bandwidth.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_track::entity::Entity;
use gwr_track::trace;

use crate::engine::Engine;
use crate::time::clock::Clock;
use crate::traits::{Runnable, SimObject};
use crate::types::SimResult;

pub struct Monitor {
    pub entity: Rc<Entity>,
    clock: Clock,
    window_size_ticks: u64,
    bytes_in_window: RefCell<usize>,
    bytes_total: RefCell<usize>,
}

impl Monitor {
    #[must_use]
    pub fn new_and_register(
        engine: &Engine,
        entity: Rc<Entity>,
        clock: Clock,
        window_size_ticks: u64,
    ) -> Rc<Self> {
        let rc_self = Rc::new(Self {
            entity,
            clock,
            window_size_ticks,
            bytes_in_window: RefCell::new(0),
            bytes_total: RefCell::new(0),
        });

        engine.register(rc_self.clone());
        rc_self
    }

    pub fn monitor<T>(&self, object: &T)
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

            trace!(self.entity ; "monitor {bytes_in_window} bytes");
        }
    }
}
