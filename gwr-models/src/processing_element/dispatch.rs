// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use async_trait::async_trait;
use gwr_engine::types::{SimError, SimResult};

use crate::processing_element::task::Task;

#[async_trait(?Send)]
pub trait Dispatch {
    fn task_by_id(&self, task_idx: usize) -> Result<Task, SimError>;
    fn set_task_active(&self, task_idx: usize) -> SimResult;
    fn set_task_completed(&self, task_idx: usize) -> SimResult;
    fn ready_task_indices(&self, pe_name: &str) -> Result<(bool, Vec<usize>), SimError>;
    async fn wait_for_change(&self);
    fn total_tasks_for_pe(&self, pe_name: &str) -> usize;
}
