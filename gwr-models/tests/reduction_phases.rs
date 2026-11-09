// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::collections::BTreeMap;

use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
use gwr_models::processing_element::program::tile::plan_tiles;
use gwr_models::processing_element::program::validate::{RegionBindings, validate_program};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
#[test]
fn streamed_row_reduction_then_broadcast_rereads_the_complete_row() {
    let program: ProgramSpec = yaml_serde::from_str(r"
inputs: [A]
outputs: [Y]
temporaries: { S: { dtype: fp32 } }
assignments:
 - output: S
   axes: [r]
   expr: { reduce: { op: sum, over: [k], value: { mul: [{ read: { tensor: A, indices: [r, k] } }, { read: { tensor: A, indices: [r, k] } }] } } }
 - output: Y
   axes: [r, k]
   expr: { mul: [{ read: { tensor: A, indices: [r, k] } }, { read: { tensor: S, indices: [r] } }] }
").unwrap();
    let view = |addr| TensorView::new_full(Tensor::new(&[2, 1025], &DataType::Fp32, addr).unwrap());
    let region = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(0)],
            outputs: vec![view(16384)],
        },
    )
    .unwrap();
    let schedule = plan_tiles(
        &region,
        Some(&TileSpec {
            sizes: BTreeMap::from([("r".into(), 1), ("k".into(), 128)]),
        }),
    )
    .unwrap();
    let tiles: Vec<_> = schedule.tiles().collect();
    assert_eq!(tiles.len(), 18);
    let task = gwr_models::processing_element::task::ProgramTaskConfig {
        pipeline: None,
        id: "reread".into(),
        program: program.clone(),
        tiling: Some(TileSpec {
            sizes: BTreeMap::from([("r".into(), 1), ("k".into(), 128)]),
        }),
        inputs: vec![view(0)],
        outputs: vec![view(16384)],
    };
    assert_eq!(
        task.planned_traffic_bytes().unwrap(),
        ((2 * 9 * 1025 + 2 * 1025) * 4, 2 * 1025 * 4)
    );
    assert_eq!(
        gwr_models::processing_element::estimate_peak_tile_sram(
            &program,
            &task.inputs,
            &task.outputs,
            task.tiling.as_ref().unwrap()
        )
        .unwrap(),
        2056
    );

    for tile in &tiles {
        assert_eq!(
            tile.reduction_chunks_for(&["k".into()])
                .map(|c| c["k"].len())
                .sum::<usize>(),
            1025
        );
    }
    assert_eq!(tiles[8].output_intervals["k"], 1024..1025);
}
#[test]
fn stable_softmax_streams_nested_elementwise_reduction_body() {
    let program: ProgramSpec = yaml_serde::from_str(r"
inputs: [A]
outputs: [Y]
temporaries: { M: { dtype: fp32 }, S: { dtype: fp32 } }
assignments:
 - output: M
   axes: [r]
   expr: { reduce: { op: max, over: [k], value: { read: { tensor: A, indices: [r, k] } } } }
 - output: S
   axes: [r]
   expr: { reduce: { op: sum, over: [k], value: { exp: [{ sub: [{ read: { tensor: A, indices: [r, k] } }, { read: { tensor: M, indices: [r] } }] }] } } }
 - output: Y
   axes: [r, k]
   expr: { div: [{ exp: [{ sub: [{ read: { tensor: A, indices: [r, k] } }, { read: { tensor: M, indices: [r] } }] }] }, { read: { tensor: S, indices: [r] } }] }
").unwrap();
    let view = |addr| TensorView::new_full(Tensor::new(&[1, 1025], &DataType::Fp32, addr).unwrap());
    let region = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(0)],
            outputs: vec![view(8192)],
        },
    )
    .unwrap();
    assert_eq!(
        plan_tiles(
            &region,
            Some(&TileSpec {
                sizes: BTreeMap::from([("k".into(), 128)])
            })
        )
        .unwrap()
        .tiles()
        .count(),
        9
    );
    let tiling = TileSpec {
        sizes: BTreeMap::from([("k".into(), 128)]),
    };
    let bytes = gwr_models::processing_element::estimate_peak_tile_sram(
        &program,
        &[view(0)],
        &[view(8192)],
        &tiling,
    )
    .unwrap();
    assert_eq!(bytes, 4112);
}

#[test]
fn materialized_local_row_cannot_be_reduced_after_only_one_output_chunk_is_produced() {
    let program: ProgramSpec = yaml_serde::from_str(
        r"
inputs: [A]
outputs: [Y]
temporaries: { T: { dtype: fp32 }, S: { dtype: fp32 } }
assignments:
 - output: T
   axes: [r,k]
   expr: { exp: [{ read: { tensor: A, indices: [r,k] } }] }
 - output: S
   axes: [r]
   expr: { reduce: { op: sum, over: [k], value: { read: { tensor: T, indices: [r,k] } } } }
 - output: Y
   axes: [r,k]
   expr: { div: [{ read: { tensor: T, indices: [r,k] } }, { read: { tensor: S, indices: [r] } }] }
",
    )
    .unwrap();
    let view = |addr| TensorView::new_full(Tensor::new(&[1, 1025], &DataType::Fp32, addr).unwrap());
    let region = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(0)],
            outputs: vec![view(8192)],
        },
    )
    .unwrap();
    assert!(
        plan_tiles(
            &region,
            Some(&TileSpec {
                sizes: BTreeMap::from([("k".into(), 128)])
            })
        )
        .is_err()
    );
}
#[test]
fn phase_specific_affine_boundary_views_report_both_reads() {
    let program:ProgramSpec=yaml_serde::from_str(r"
inputs: [A]
outputs: [Y]
metadata_inputs: []
extents: { i: 7 }
temporaries: { T: { dtype: fp32 } }
assignments:
 - output: T
   axes: [i]
   expr: { read: { tensor: A, indices: [{ affine: { terms: { i: 1 }, offset: 0 } }] } }
 - output: Y
   axes: [i]
   expr: { add: [{ read: { tensor: T, indices: [i] } }, { read: { tensor: A, indices: [{ affine: { terms: { i: 1 }, offset: 1 } }] } }] }
").unwrap();
    let view = |shape: &[usize], addr| {
        TensorView::new_full(Tensor::new(shape, &DataType::Fp32, addr).unwrap())
    };
    let config = gwr_models::processing_element::task::ProgramTaskConfig {
        pipeline: None,
        id: "affine_phases".into(),
        program,
        tiling: Some(TileSpec {
            sizes: BTreeMap::from([("i".into(), 3)]),
        }),
        inputs: vec![view(&[8], 0)],
        outputs: vec![view(&[7], 64)],
    };
    assert_eq!(config.planned_traffic_bytes().unwrap(), (56, 28));
}
