// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::processing_element::estimate_peak_tile_sram;
use gwr_models::processing_element::program::syntax::TileSpec;
use gwr_models::processing_element::task::ProgramTaskConfig;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};

#[test]
fn packed_offset_reduction_sizes_sram_independently_of_transfer_bytes() {
    for index in ["k", "{ affine: { terms: { k: 1 }, offset: 0 } }"] {
        let program = yaml_serde::from_str(&format!(
            r"
inputs: [A]
outputs: [Y]
extents: {{k: 16}}
assignments:
  - output: Y
    axes: []
    expr:
      reduce:
        op: sum
        over: [k]
        value:
          read: {{tensor: A, indices: [{index}]}}
"
        ))
        .unwrap();
        let tiling = TileSpec {
            sizes: BTreeMap::from([("k".into(), 16)]),
        };
        let config = ProgramTaskConfig {
            id: "packed reduction".into(),
            program,
            tiling: Some(tiling.clone()),
            pipeline: None,
            inputs: vec![
                TensorView::new(Tensor::new(&[17], &DataType::Int4, 0).unwrap(), &[16], &[1])
                    .unwrap(),
            ],
            outputs: vec![TensorView::new_full(
                Tensor::new(&[], &DataType::Int4, 1024).unwrap(),
            )],
        };
        assert_eq!(config.planned_traffic_bytes().unwrap(), (9, 1), "{index}");
        // Sixteen selected nibbles occupy eight SRAM bytes even when the
        // source spans nine bytes. The reduction has one eight-byte
        // accumulator.
        assert_eq!(
            estimate_peak_tile_sram(&config.program, &config.inputs, &config.outputs, &tiling)
                .unwrap(),
            16,
            "{index}"
        );
    }
}
