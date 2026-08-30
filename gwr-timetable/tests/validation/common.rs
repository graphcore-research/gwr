// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::test_helpers::start_test;
use gwr_engine::types::SimError;
use gwr_models::processing_element::MachineOpCounts;
pub(crate) use gwr_models::processing_element::dispatch::Dispatch;
pub(crate) use gwr_models::processing_element::operators::dtype::DataType;
pub(crate) use gwr_models::processing_element::operators::{OperatorCustom, OperatorMaxPool};
pub(crate) use gwr_models::processing_element::task::ComputeOp;
use gwr_platform::Platform;
pub(crate) use gwr_timetable::Timetable;
pub(crate) use gwr_timetable::timetable_file::{
    EdgeKind, EdgeSection, NodeSection, TensorConfigSection, TensorViewSection, TimetableFile,
};
use gwr_track::entity::Entity;

pub(crate) fn create_default_timetable_file() -> (Rc<Entity>, Rc<Platform>, TimetableFile) {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    (
        engine.top().clone(),
        Rc::new(
            Platform::from_string(
                &engine,
                &clock,
                "
memory_maps:
  - name: default
    devices:
      - name: hbm0

processing_elements:
  - name: pe0
    memory_map: default
    config:

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config:
      capacity_bytes: 0x1000_0000
",
            )
            .unwrap(),
        ),
        TimetableFile::from_string(
            "
nodes:
  - id: tensor0
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [3, 10, 10]

  - id: tensor1
    kind: tensor
    config:
      addr: 0x1000
      dtype: fp32
      shape: [3, 10, 10]

  - id: add0
    kind: compute
    op: add
    pe: pe0
    input_views:
      -
      -
    output_views:
      -

  - id: tensor2
    kind: tensor
    config:
      addr: 0x2000
      dtype: fp32
      shape: [3, 10, 10]

edges:
  - from: tensor0
    to: add0.0
    kind: data

  - from: tensor1
    to: add0.1
    kind: data

  - from: add0
    to: tensor2
    kind: data
",
        )
        .unwrap(),
    )
}

pub(crate) fn build_timetable(
    top: &Rc<Entity>,
    file: TimetableFile,
    platform: &Rc<Platform>,
) -> Result<Timetable, SimError> {
    Timetable::new(top, file.into_graph()?, platform)
}

pub(crate) fn timetable(nodes: Vec<NodeSection>, edges: Vec<EdgeSection>) -> TimetableFile {
    TimetableFile { nodes, edges }
}

pub(crate) fn tensor(id: &str, addr: u64, dtype: DataType, shape: &[usize]) -> NodeSection {
    NodeSection::Tensor {
        id: id.to_string(),
        config: TensorConfigSection {
            addr,
            dtype,
            shape: shape.to_vec(),
        },
    }
}

pub(crate) fn compute(
    id: &str,
    input_views: Vec<Option<TensorViewSection>>,
    output_views: Vec<Option<TensorViewSection>>,
) -> NodeSection {
    NodeSection::Compute {
        id: id.to_string(),
        op: ComputeOp::Custom(OperatorCustom {
            name: None,
            machine_ops: MachineOpCounts::default(),
        }),
        pe: None,
        input_views,
        output_views,
    }
}

pub(crate) fn view(offsets: &[usize], shape: &[usize]) -> Option<TensorViewSection> {
    Some(TensorViewSection {
        offsets: offsets.to_vec(),
        shape: shape.to_vec(),
    })
}

pub(crate) fn data_edge(from: &str, to: &str) -> EdgeSection {
    edge(from, to, EdgeKind::Data)
}

pub(crate) fn control_edge(from: &str, to: &str) -> EdgeSection {
    edge(from, to, EdgeKind::Control)
}

fn edge(from: &str, to: &str, kind: EdgeKind) -> EdgeSection {
    EdgeSection {
        from: from.to_string(),
        to: to.to_string(),
        kind,
    }
}
