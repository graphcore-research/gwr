// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_timetable::timetable_file::TimetableFile;

#[test]
fn accepts_tile_axis_missing_from_an_independent_assignment() {
    let file = TimetableFile::from_string(
        r"
nodes:
  - { id: tokens, kind: tensor, config: { addr: 0, dtype: fp32, shape: [4] } }
  - { id: weight, kind: tensor, config: { addr: 64, dtype: fp32, shape: [4, 8] } }
  - id: embedding
    kind: compute
    program:
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
    tiling: { sizes: { t: 2, h: 4 } }
  - { id: hidden, kind: tensor, config: { addr: 256, dtype: fp32, shape: [4, 8] } }
edges:
  - { from: tokens, to: embedding.0, kind: data }
  - { from: weight, to: embedding.1, kind: data }
  - { from: embedding, to: hidden, kind: data }
",
    )
    .unwrap();
    file.validate().unwrap();
}

#[test]
fn rejects_unbound_program_input() {
    let file = TimetableFile::from_string(
        r"
nodes:
  - { id: input, kind: tensor, config: { addr: 0, dtype: fp32, shape: [4, 4] } }
  - id: compute
    kind: compute
    program:
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
  - { id: output, kind: tensor, config: { addr: 64, dtype: fp32, shape: [4, 4] } }
edges:
  - { from: input, to: compute, kind: data }
  - { from: compute, to: output, kind: data }
",
    )
    .unwrap();
    let error = file.validate().unwrap_err();
    assert!(
        error.to_string().contains("has an unbound program input"),
        "{error}"
    );
}

#[test]
fn rejects_overflowing_window_index() {
    let file = TimetableFile::from_string(&format!(
        r"
nodes:
  - {{ id: input, kind: tensor, config: {{ addr: 0, dtype: int8, shape: [1] }} }}
  - id: window
    kind: compute
    program:
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
  - {{ id: output, kind: tensor, config: {{ addr: 2, dtype: int8, shape: [1] }} }}
edges:
  - {{ from: input, to: window, kind: data }}
  - {{ from: window, to: output, kind: data }}
",
        max = usize::MAX,
    ))
    .unwrap();
    let error = file.validate().unwrap_err();
    assert!(error.to_string().contains("overflow"), "{error}");
}
