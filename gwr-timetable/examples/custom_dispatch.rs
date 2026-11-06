// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

// ANCHOR: example
use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::traits::Event;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::program::syntax::ProgramSpec;
use gwr_models::processing_element::program::{PreparedProgram, ProgramBindings, TensorSpec};
use gwr_models::processing_element::task::Task;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use gwr_platform::Platform;

// A single task for the platform's only PE. Wake it when completion changes.
struct OneTask {
    task: Task,
    active: Cell<bool>,
    completed: Cell<bool>,
    changed: Repeated<()>,
}

#[async_trait(?Send)]
impl Dispatch for OneTask {
    fn task_by_id(&self, _index: usize) -> Result<Task, SimError> {
        Ok(self.task.clone())
    }
    fn set_task_active(&self, _index: usize) -> SimResult {
        self.active.set(true);
        Ok(())
    }
    fn set_task_completed(&self, _index: usize) -> SimResult {
        self.completed.set(true);
        self.changed.notify();
        Ok(())
    }
    fn ready_task_indices(&self, _pe: &str) -> Result<(bool, Vec<usize>), SimError> {
        let ready = if self.active.get() { vec![] } else { vec![0] };
        Ok((self.completed.get(), ready))
    }
    async fn wait_for_change(&self) {
        self.changed.listen().await;
    }
    fn total_tasks_for_pe(&self, _pe: &str) -> usize {
        1
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Platform::from_file(
        &engine,
        &clock,
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/compute_region_platform.yaml"),
    )?;
    let spec: ProgramSpec = yaml_serde::from_str(
        "inputs: [A, B]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      add:
        - { read: { tensor: A, indices: [i] } }
        - { read: { tensor: B, indices: [i] } }
",
    )?;
    let view = |addr| Tensor::new(&[8], &DataType::Fp32, addr).map(TensorView::new_full);
    // Stage 1 owns views in the order of spec.inputs and spec.outputs.
    let program = Rc::new(PreparedProgram::new(
        &spec,
        ProgramBindings {
            inputs: vec![view(0x1000)?, view(0x2000)?],
            outputs: vec![view(0x3000)?],
        },
        None,
    )?);
    let result: &TensorSpec = program.value("Y").expect("declared output");
    assert_eq!(result.shape, [8]);
    // Stage 2 checks capabilities, mapped addresses, and SRAM for this exact
    // PE.
    let task = platform.pe("pe0")?.prepare_program("add".into(), program)?;
    let dispatcher = Rc::new(OneTask {
        task: Task::PreparedProgram { task },
        active: Cell::new(false),
        completed: Cell::new(false),
        changed: Repeated::new(()),
    });
    let dispatch: Rc<dyn Dispatch> = dispatcher.clone();
    platform.attach_dispatcher(&dispatch);
    engine.run()?;
    assert!(dispatcher.completed.get());
    Ok(())
}
// ANCHOR_END: example
