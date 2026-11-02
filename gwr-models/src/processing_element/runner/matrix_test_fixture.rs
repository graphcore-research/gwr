// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Private runner extension of the native matrix fixture.
use std::cell::Cell;

use async_trait::async_trait;
use futures::future::pending;
use gwr_engine::engine::Engine;

use super::*;
use crate::memory::Memory;
use crate::memory::memory_access::MemoryAccess;
use crate::processing_element::ProcessingElementConfig;
use crate::processing_element::dispatch::Dispatch;
pub use crate::processing_element::matrix_test_fixture::*;

pub fn matrix_runner_fixture(
    engine: &mut Engine,
    config: &ProcessingElementConfig,
) -> (TaskRunner, Rc<Memory<MemoryAccess>>, Rc<Completed>) {
    let (pe, memory) = processing_element_and_memory_fixture(engine, config, "memory");
    let done = Rc::new(Completed(Cell::new(0)));
    let dispatcher: Dispatcher = done.clone();
    pe.set_dispatcher(&dispatcher);
    let runner = TaskRunner {
        entity: pe.entity.clone(),
        clock: engine.default_clock(),
        dispatcher,
        lsu: pe.lsu.clone(),
        compute_model: pe.compute_model.clone(),
        compute_scheduler: pe.compute_scheduler.clone(),
        sram: pe.sram.clone(),
        sram_bytes: config.sram_bytes,
        stats: pe.stats.clone(),
        activity_lanes: pe.activity_lanes.clone(),
        flop_monitor: None,
    };
    (runner, memory, done)
}

pub struct Completed(pub Cell<usize>);

#[async_trait(?Send)]
impl Dispatch for Completed {
    fn task_by_id(&self, _: usize) -> Result<Task, SimError> {
        unreachable!()
    }

    fn set_task_active(&self, _: usize) -> SimResult {
        Ok(())
    }

    fn set_task_completed(&self, _: usize) -> SimResult {
        self.0.set(self.0.get() + 1);
        Ok(())
    }

    fn ready_task_indices(&self, _: &str) -> Result<(bool, Vec<usize>), SimError> {
        Ok((true, vec![]))
    }

    async fn wait_for_change(&self) {
        pending::<()>().await;
    }

    fn total_tasks_for_pe(&self, _: &str) -> usize {
        0
    }
}
