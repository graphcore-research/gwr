// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use yaml_serde::from_str;

use crate::processing_element::estimate_peak_tile_sram;
use crate::processing_element::program::syntax::{ExprSpec, IndexSpec, ProgramSpec, TileSpec};
use crate::processing_element::program::tile::{plan_tiles, validate_tiling};
use crate::processing_element::program::validate::{
    RegionBindings, ValidatedRegion, validate_program,
};
use crate::processing_element::task::ProgramTaskConfig;
use crate::processing_element::tensors::dtype::DataType;
use crate::processing_element::tensors::{Tensor, TensorView};

fn elementwise_region() -> ValidatedRegion {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      add:
        - read: { tensor: A, indices: [i] }
        - read: { tensor: B, indices: [i] }
",
    )
    .unwrap();
    validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(&[10], 0), view(&[10], 100)],
            outputs: vec![view(&[10], 200)],
        },
    )
    .unwrap()
}

fn view(shape: &[usize], addr: u64) -> TensorView {
    TensorView::new_full(Tensor::new(shape, &DataType::Fp32, addr).unwrap())
}

#[test]
fn affine_sum_window_plans_traffic_for_valid_input_elements() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [Y]
extents: { y: 3, k: 3 }
assignments:
  - output: Y
    axes: [y]
    expr:
      reduce:
        op: sum
        over: [k]
        value:
          mul:
            - read:
                tensor: A
                indices:
                  - affine:
                      terms: { y: 1, k: 1 }
                      offset: -1
                bounds: exclude
            - read: { tensor: B, indices: [k] }
",
    )
    .unwrap();
    let task = ProgramTaskConfig {
        id: "conv_window".into(),
        program,
        tiling: Some(TileSpec {
            sizes: BTreeMap::from([("y".into(), 1)]),
        }),
        inputs: vec![view(&[3], 0), view(&[3], 32)],
        outputs: vec![view(&[3], 64)],
    };
    assert_eq!(task.planned_traffic_bytes().unwrap(), (64, 12));
}

#[test]
fn unread_bound_input_has_no_planned_memory_traffic() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, dependency]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      read: { tensor: A, indices: [i] }
",
    )
    .unwrap();
    let task = ProgramTaskConfig {
        id: "dependency_only".into(),
        program,
        tiling: None,
        inputs: vec![view(&[4], 0), view(&[4], 32)],
        outputs: vec![view(&[4], 64)],
    };
    assert_eq!(task.planned_traffic_bytes().unwrap(), (16, 16));
}

#[test]
fn splits_parallel_and_reduction_axes_with_tails() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [Y]
assignments:
  - output: Y
    axes: [m, n]
    expr:
      reduce:
        op: sum
        over: [k]
        value:
          mul:
            - read: { tensor: A, indices: [m, k] }
            - read: { tensor: B, indices: [k, n] }
",
    )
    .unwrap();
    let region = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(&[10, 5], 0), view(&[5, 9], 1000)],
            outputs: vec![view(&[10, 9], 2000)],
        },
    )
    .unwrap();
    let schedule = plan_tiles(
        &region,
        Some(&TileSpec {
            sizes: BTreeMap::from([("m".into(), 4), ("n".into(), 4), ("k".into(), 2)]),
        }),
    )
    .unwrap();
    let tiles = schedule.tiles().collect::<Vec<_>>();
    assert_eq!(tiles.len(), 9);
    assert_eq!(tiles[0].output_intervals["m"], 0..4);
    assert_eq!(tiles[0].output_intervals["n"], 0..4);
    assert_eq!(tiles[8].output_intervals["m"], 8..10);
    assert_eq!(tiles[8].output_intervals["n"], 8..9);
    let chunks = tiles[0].reduction_chunks().collect::<Vec<_>>();
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0]["k"], 0..2);
    assert_eq!(chunks[1]["k"], 2..4);
    assert_eq!(chunks[2]["k"], 4..5);
}

#[test]
fn no_tiling_is_one_complete_tile() {
    let region = elementwise_region();
    let full = plan_tiles(&region, None)
        .unwrap()
        .tiles()
        .collect::<Vec<_>>();
    assert_eq!(full.len(), 1);
    assert_eq!(full[0].output_intervals["i"], 0..10);
}

#[test]
fn rejects_zero_tile_size() {
    assert!(
        plan_tiles(
            &elementwise_region(),
            Some(&TileSpec {
                sizes: BTreeMap::from([("i".into(), 0)])
            })
        )
        .is_err()
    );
}

#[test]
fn rejects_unknown_tile_axis() {
    assert!(
        plan_tiles(
            &elementwise_region(),
            Some(&TileSpec {
                sizes: BTreeMap::from([("other".into(), 4)])
            })
        )
        .is_err()
    );
}

#[test]
fn independent_assignments_can_tile_different_output_axes() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [Y, Z]
assignments:
  - output: Y
    axes: [i]
    expr:
      add:
        - read: { tensor: A, indices: [i] }
        - read: { tensor: A, indices: [i] }
  - output: Z
    axes: [j]
    expr:
      mul:
        - read: { tensor: B, indices: [j] }
        - read: { tensor: B, indices: [j] }
",
    )
    .unwrap();
    let region = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(&[8], 0), view(&[8], 64)],
            outputs: vec![view(&[8], 128), view(&[8], 192)],
        },
    )
    .unwrap();
    let tiling = TileSpec {
        sizes: BTreeMap::from([("i".into(), 4)]),
    };
    validate_tiling(&region, Some(&tiling)).unwrap();
    let task = ProgramTaskConfig {
        id: "independent_axes".into(),
        program,
        tiling: Some(tiling),
        inputs: region.bindings.inputs,
        outputs: region.bindings.outputs,
    };
    // Each independent assignment visits its own eight-element vector once.
    assert_eq!(
        task.planned_traffic_bytes().unwrap(),
        (2 * 8 * 4, 2 * 8 * 4)
    );
}

#[test]
fn packed_affine_sram_estimate_is_unchanged_by_view_offsets_and_strides() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y]
extents: { i: 16 }
assignments:
 - output: Y
   axes: [i]
   expr:
     convert:
       - read:
           tensor: A
           indices:
             - affine:
                 terms: { i: 1 }
                 offset: 0
",
    )
    .unwrap();
    let input = TensorView::new_full(Tensor::new(&[64], &DataType::Fp4, 0).unwrap());
    let output = TensorView::new_full(Tensor::new(&[40], &DataType::Bf16, 64).unwrap());
    for stride in [1, 2] {
        for offset in [0, 15] {
            let input_tile =
                TensorView::new(input.tensor().clone(), &[16 * stride], &[offset]).unwrap();
            let output_tile = TensorView::new(output.tensor().clone(), &[16], &[offset]).unwrap();
            let mut tile_program = program.clone();
            let ExprSpec::Call { args, .. } = &mut tile_program.assignments[0].expr else {
                unreachable!()
            };
            let ExprSpec::Read { indices, .. } = &mut args[0] else {
                unreachable!()
            };
            let IndexSpec::Affine { affine } = &mut indices[0] else {
                unreachable!()
            };
            affine.terms.insert("i".into(), stride);
            assert_eq!(
                estimate_peak_tile_sram(
                    &tile_program,
                    &[input_tile],
                    &[output_tile],
                    &TileSpec::default(),
                )
                .unwrap(),
                40,
            );
        }
    }
}

#[test]
fn trillion_element_packed_conversion_uses_a_bounded_full_tile_sram_probe() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y]
extents: { k: 1000000000000 }
assignments:
 - output: Y
   axes: [k]
   expr:
     convert:
       - read:
           tensor: A
           indices:
             - affine:
                 terms: { k: 1 }
                 offset: 0
",
    )
    .unwrap();
    let elements = 1_000_000_000_000usize;
    let input = TensorView::new_full(Tensor::new(&[elements], &DataType::Fp4, 0).unwrap());
    let output =
        TensorView::new_full(Tensor::new(&[elements], &DataType::Bf16, 1_000_000_000_000).unwrap());
    assert_eq!(
        estimate_peak_tile_sram(
            &program,
            &[input],
            &[output],
            &TileSpec {
                sizes: BTreeMap::from([("k".into(), 128)])
            }
        )
        .unwrap(),
        320
    );
}
