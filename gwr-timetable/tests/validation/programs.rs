// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_timetable::timetable_file::NodeSection;

use super::common::{data_edge, tensor, timetable};

#[test]
fn rejects_tile_axis_missing_from_an_assignment_before_simulation() {
    let program: ProgramSpec = yaml_serde::from_str(
        r"
inputs: [A, B]
outputs: [Y]
temporaries:
  token_slice:
    dtype: fp32
assignments:
  - output: token_slice
    axes: [t]
    expr:
      read:
        tensor: A
        indices: [t]
  - output: Y
    axes: [t, h]
    expr:
      add:
        - read:
            tensor: B
            indices: [t, h]
        - read:
            tensor: B
            indices: [t, h]
",
    )
    .unwrap();
    let file = timetable(
        vec![
            tensor("tokens", 0, DataType::Fp32, &[4]),
            tensor("weight", 64, DataType::Fp32, &[4, 8]),
            NodeSection::Compute {
                pipeline: None,
                id: "embedding".into(),
                program: program.into(),
                tiling: Some(TileSpec {
                    sizes: BTreeMap::from([("t".into(), 2), ("h".into(), 4)]),
                }),
                pe: None,
                input_views: None,
                output_views: None,
            },
            tensor("hidden", 256, DataType::Fp32, &[4, 8]),
        ],
        vec![
            data_edge("tokens", "embedding.0"),
            data_edge("weight", "embedding.1"),
            data_edge("embedding", "hidden"),
        ],
    );
    let error = file.validate().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("tile axis h must occur in every assignment output"),
        "{error}"
    );
}

#[test]
fn rejects_unbound_program_input() {
    let program: ProgramSpec = yaml_serde::from_str(
        r"
inputs: [A, B]
outputs: [Y]
assignments:
  - output: Y
    axes: [m, n]
    expr:
      add:
        - read:
            tensor: A
            indices: [m, n]
        - read:
            tensor: B
            indices: [m, n]
",
    )
    .unwrap();
    let file = timetable(
        vec![
            tensor("input", 0, DataType::Fp32, &[4, 4]),
            NodeSection::Compute {
                pipeline: None,
                id: "compute".into(),
                program: program.into(),
                tiling: None,
                pe: None,
                input_views: None,
                output_views: None,
            },
            tensor("output", 64, DataType::Fp32, &[4, 4]),
        ],
        vec![
            data_edge("input", "compute"),
            data_edge("compute", "output"),
        ],
    );
    let error = file.validate().unwrap_err();
    assert!(
        error.to_string().contains("has an unbound program input"),
        "{error}"
    );
}

#[test]
fn rejects_overflowing_window_index() {
    let program: ProgramSpec = yaml_serde::from_str(&format!(
        r"
inputs: [A]
outputs: [Y]
extents:
  o: {max}
  k: 2
assignments:
  - output: Y
    axes: [o]
    expr:
      reduce:
        op: max
        over: [k]
        value:
          read:
            tensor: A
            indices:
              - affine:
                  terms:
                    o: {max}
                    k: 1
                  offset: 0
            bounds: exclude
",
        max = usize::MAX,
    ))
    .unwrap();
    let file = timetable(
        vec![
            tensor("input", 0, DataType::Int8, &[1]),
            NodeSection::Compute {
                pipeline: None,
                id: "window".into(),
                program: program.into(),
                tiling: None,
                pe: None,
                input_views: None,
                output_views: None,
            },
            tensor("output", 2, DataType::Int8, &[1]),
        ],
        vec![data_edge("input", "window"), data_edge("window", "output")],
    );
    let error = file.validate().unwrap_err();
    assert!(error.to_string().contains("overflow"), "{error}");
}
