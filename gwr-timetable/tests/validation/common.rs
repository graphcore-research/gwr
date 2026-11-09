// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::rc::Rc;

use gwr_engine::test_helpers::start_test;
use gwr_engine::types::SimError;
use gwr_models::processing_element::program::syntax::ProgramSpec;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::{
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
      - hbm0

operation_classes:
  add: { operations: [add], arity: 2 }

processing_elements:
  - name: pe0
    memory_map: default
    config:
      compute:
        engines:
          vector:
            capabilities: [{ class: add, ops_per_tick: 256 }]

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
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [d0, d1, d2]
          expr:
            add:
              - read:
                  tensor: A
                  indices: [d0, d1, d2]
              - read:
                  tensor: B
                  indices: [d0, d1, d2]

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
    TimetableFile {
        atomic_effects: None,
        programs: BTreeMap::default(),
        nodes,
        edges,
    }
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
    compute_with_program(
        id,
        ProgramSpec {
            metadata_inputs: vec![],
            indexed_metadata_inputs: vec![],
            local_round_policy: None,
            inputs: (0..input_views.len())
                .map(|index| format!("input{index}"))
                .collect(),
            outputs: (0..output_views.len())
                .map(|index| format!("output{index}"))
                .collect(),
            temporaries: std::collections::BTreeMap::default(),
            assignments: vec![],
            extents: std::collections::BTreeMap::default(),
        },
        input_views,
        output_views,
    )
}

pub(crate) fn compute_with_program(
    id: &str,
    program: ProgramSpec,
    input_views: Vec<Option<TensorViewSection>>,
    output_views: Vec<Option<TensorViewSection>>,
) -> NodeSection {
    NodeSection::Compute {
        pipeline: None,
        id: id.to_string(),
        program: program.into(),
        tiling: None,
        pe: None,
        input_views: Some(input_views),
        output_views: Some(output_views),
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
