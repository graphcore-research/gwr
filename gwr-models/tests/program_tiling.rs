// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
use gwr_models::processing_element::program::tile::plan_tiles;
use gwr_models::processing_element::program::validate::{RegionBindings, validate_program};
use gwr_models::processing_element::task::ProgramTaskConfig;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};

#[test]
fn affine_sum_window_counts_only_valid_input_elements() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A, B]\noutputs: [Y]\nextents: { y: 3, k: 3 }\nassignments:\n  - output: Y\n    axes: [y]\n    expr: { reduce: { op: sum, over: [k], value: { mul: [{ read: { tensor: A, indices: [{ affine: { terms: { y: 1, k: 1 }, offset: -1 } }], bounds: exclude } }, { read: { tensor: B, indices: [k] } }] } } }\n",
    ).unwrap();
    let task = ProgramTaskConfig {
        pipeline: None,
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
fn unread_bound_input_has_no_memory_traffic() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A, dependency]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: { read: { tensor: A, indices: [i] } }\n",
    ).unwrap();
    let task = ProgramTaskConfig {
        pipeline: None,
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
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A, B]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [m, n]\n    expr: { reduce: { op: sum, over: [k], value: { mul: [{ read: { tensor: A, indices: [m, k] } }, { read: { tensor: B, indices: [k, n] } }] } } }\n",
    ).unwrap();
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
fn no_tiling_is_one_complete_tile_and_invalid_sizes_fail() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A, B]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }\n",
    ).unwrap();
    let region = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(&[10], 0), view(&[10], 100)],
            outputs: vec![view(&[10], 200)],
        },
    )
    .unwrap();
    let full = plan_tiles(&region, None)
        .unwrap()
        .tiles()
        .collect::<Vec<_>>();
    assert_eq!(full.len(), 1);
    assert_eq!(full[0].output_intervals["i"], 0..10);
    for (axis, size) in [("i", 0), ("other", 4)] {
        assert!(
            plan_tiles(
                &region,
                Some(&TileSpec {
                    sizes: BTreeMap::from([(axis.into(), size)])
                })
            )
            .is_err()
        );
    }
}

#[test]
fn rejects_a_tiled_axis_missing_from_an_independent_assignment() {
    let program: ProgramSpec = yaml_serde::from_str(
        r"
inputs: [A, B]
outputs: [Y, Z]
assignments:
  - output: Y
    axes: [i]
    expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: A, indices: [i] } }] }
  - output: Z
    axes: [j]
    expr: { mul: [{ read: { tensor: B, indices: [j] } }, { read: { tensor: B, indices: [j] } }] }
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
    let error = plan_tiles(
        &region,
        Some(&TileSpec {
            sizes: BTreeMap::from([("i".into(), 4)]),
        }),
    )
    .unwrap_err();
    assert!(error.0.contains("every assignment output"), "{error:?}");
}

fn view(shape: &[usize], addr: u64) -> TensorView {
    TensorView::new_full(Tensor::new(shape, &DataType::Fp32, addr).unwrap())
}

#[test]
fn trillion_element_packed_conversion_uses_a_bounded_full_tile_sram_probe() {
    let program:ProgramSpec=yaml_serde::from_str(r"
inputs: [A]
outputs: [Y]
extents: { k: 1000000000000 }
assignments:
 - output: Y
   axes: [k]
   expr: { convert: [{ read: { tensor: A, indices: [{ affine: { terms: { k: 1 }, offset: 0 } }] } }] }
").unwrap();
    let elements = 1_000_000_000_000usize;
    let input = TensorView::new_full(Tensor::new(&[elements], &DataType::Fp4, 0).unwrap());
    let output =
        TensorView::new_full(Tensor::new(&[elements], &DataType::Bf16, 1_000_000_000_000).unwrap());
    assert_eq!(
        gwr_models::processing_element::estimate_peak_tile_sram(
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
