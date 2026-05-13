// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Monitor for PE FLOP throughput.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::Runnable;
use gwr_engine::types::SimResult;
use gwr_track::entity::{Entity, EntityMonitor};

pub struct FlopMonitor {
    entity: EntityMonitor,
    clock: Clock,
    window_size_ticks: u64,
    flops_by_window: RefCell<HashMap<u64, f64>>,
    next_window_idx: RefCell<u64>,
}

impl FlopMonitor {
    #[must_use]
    pub fn new_and_register(
        engine: &Engine,
        entity: &Rc<Entity>,
        clock: &Clock,
        window_size_ticks: u64,
    ) -> Rc<Self> {
        let window_size_ticks = window_size_ticks.max(1);
        let flop_entity = EntityMonitor::new(entity, "gflops");

        let rc_self = Rc::new(Self {
            entity: flop_entity,
            clock: clock.clone(),
            window_size_ticks,
            flops_by_window: RefCell::new(HashMap::new()),
            next_window_idx: RefCell::new(0),
        });

        engine.register(rc_self.clone());
        rc_self
    }

    pub fn record_interval(&self, duration_ticks: u64, total_flops: f64) {
        if duration_ticks == 0 {
            return;
        }

        let start_tick = self.clock.tick_now().tick();
        for (window_idx, flops_in_window) in split_flops_across_windows(
            start_tick,
            duration_ticks,
            total_flops,
            self.window_size_ticks,
        ) {
            *self
                .flops_by_window
                .borrow_mut()
                .entry(window_idx)
                .or_insert(0.0) += flops_in_window;
        }
    }

    fn emit_window(&self, window_idx: u64, duration_ticks: u64) {
        if duration_ticks == 0 {
            return;
        }

        let flops = self
            .flops_by_window
            .borrow_mut()
            .remove(&window_idx)
            .unwrap_or(0.0);
        let duration_s = duration_ticks as f64 / (self.clock.freq_mhz() * 1e6);
        let gflops_per_second = if duration_s > 0.0 {
            flops / duration_s / 1e9
        } else {
            0.0
        };
        self.entity.track_value(gflops_per_second);
    }
}

impl Drop for FlopMonitor {
    fn drop(&mut self) {
        let now_tick = self.clock.tick_now().tick();
        let next_window_idx = *self.next_window_idx.borrow();
        let window_start_tick = next_window_idx * self.window_size_ticks;
        if now_tick > window_start_tick {
            self.emit_window(next_window_idx, now_tick - window_start_tick);
        }
    }
}

#[async_trait(?Send)]
impl Runnable for FlopMonitor {
    async fn run(&self) -> SimResult {
        loop {
            self.clock.wait_ticks_or_exit(self.window_size_ticks).await;
            let window_idx = *self.next_window_idx.borrow();
            self.emit_window(window_idx, self.window_size_ticks);
            *self.next_window_idx.borrow_mut() += 1;
        }
    }
}

fn split_flops_across_windows(
    start_tick: u64,
    duration_ticks: u64,
    total_flops: f64,
    window_size_ticks: u64,
) -> Vec<(u64, f64)> {
    let end_tick = start_tick + duration_ticks;
    let mut tick = start_tick;
    let mut split = Vec::new();

    let flops_per_tick = total_flops / duration_ticks as f64;

    while tick < end_tick {
        let window_idx = tick / window_size_ticks;
        let next_window_start_tick = (window_idx + 1) * window_size_ticks;
        let window_end_tick = next_window_start_tick.min(end_tick);
        let window_ticks = window_end_tick - tick;
        let flops = window_ticks as f64 * flops_per_tick;

        split.push((window_idx, flops));
        tick = window_end_tick;
    }

    split
}

#[cfg(test)]
mod tests {
    use super::split_flops_across_windows;

    #[test]
    fn split_flops_when_window_is_smaller_than_activity() {
        assert_eq!(
            split_flops_across_windows(2, 10, 100.0, 4),
            vec![(0, 20.0), (1, 40.0), (2, 40.0)]
        );
    }

    #[test]
    fn split_flops_when_window_is_larger_than_activity() {
        assert_eq!(
            split_flops_across_windows(3, 4, 100.0, 16),
            vec![(0, 100.0)]
        );
    }

    #[test]
    fn split_flops_start_after_window_0() {
        assert_eq!(
            split_flops_across_windows(110, 25, 100.0, 20),
            vec![(5, 40.0), (6, 60.0)]
        );
    }
}
