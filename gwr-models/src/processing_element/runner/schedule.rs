// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use futures::future::LocalBoxFuture;
use gwr_engine::types::SimResult;

use super::TaskRunner;
use super::tile::build_tile_storage;
use crate::processing_element::program::schedule::{
    Intervals, ScopedSchedule, ScopedStep, build_group_tile, scope_intervals,
};
use crate::processing_element::task::PreparedTask;

impl TaskRunner {
    pub(super) async fn run_scoped_program(
        &self,
        task: &PreparedTask,
        schedule: &ScopedSchedule,
    ) -> SimResult {
        let _reservation = self.sram.reserve(task.peak_sram_bytes()).await?;
        {
            let mut stats = self.stats.borrow_mut();
            stats.peak_sram_bytes = stats.peak_sram_bytes.max(self.sram.used());
        }
        self.run_scope(task, schedule, &BTreeMap::new()).await
    }
    fn run_scope<'a>(
        &'a self,
        task: &'a PreparedTask,
        scope: &'a ScopedSchedule,
        outer: &'a Intervals,
    ) -> LocalBoxFuture<'a, SimResult> {
        Box::pin(async move {
            let program = task.program();
            for intervals in scope_intervals(scope, outer, &program.facts) {
                for step in &scope.steps {
                    match step {
                        ScopedStep::Tiles(child) => self.run_scope(task, child, &intervals).await?,
                        ScopedStep::Group(indices) => {
                            let tile = build_group_tile(indices, &intervals, program)?;
                            let plan = program.group(indices);
                            let storage = build_tile_storage(plan, &tile, usize::MAX)?;
                            let group = self.activity_lanes.create_group(&format!(
                                "{} tile {:?}",
                                task.id(),
                                tile.output_intervals
                            ));
                            self.run_tile_actions(task, plan, tile, group, storage)
                                .await?;
                        }
                    }
                }
            }
            Ok(())
        })
    }
}
