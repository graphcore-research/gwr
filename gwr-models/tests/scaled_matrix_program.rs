// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::processing_element::program::syntax::ProgramSpec;
use gwr_models::processing_element::program::validate::{RegionBindings, validate_program};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};

fn fixture(format: &str, layout: &str, k: usize) -> (yaml_serde::Value, RegionBindings) {
    let nv = format == "nvfp4";
    let block = if nv { 16 } else { 32 };
    let blocks = k.div_ceil(block);
    let scales = if layout == "padded128x4" {
        vec![128, blocks.div_ceil(4) * 4]
    } else {
        vec![2, blocks]
    };
    let view = |shape: &[usize], dtype, addr| {
        TensorView::new_full(Tensor::new(shape, &dtype, addr).unwrap())
    };
    let mut yaml: yaml_serde::Value = yaml_serde::from_str(&format!(r"
inputs: [A, B, SA, SB, GA, GB]
metadata_inputs: [SA, SB, GA, GB]
outputs: [Y]
assignments:
 - output: Y
   axes: [m, n]
   expr:
    matrix_mac:
     execution: {{instruction: {format}, accumulator: fp32, sparsity: dense}}
     over: [k]
     args:
      - {{read: {{tensor: A, indices: [m, k]}}}}
      - {{read: {{tensor: B, indices: [n, k]}}}}
     quantization:
      - {{format: {format}, block_size: {block}, block_scales: SA, global_scale: GA, layout: {layout}}}
      - {{format: {format}, block_size: {block}, block_scales: SB, global_scale: GB, layout: {layout}}}
")).unwrap();
    if !nv {
        for operand in yaml["assignments"][0]["expr"]["matrix_mac"]["quantization"]
            .as_sequence_mut()
            .unwrap()
        {
            operand
                .as_mapping_mut()
                .unwrap()
                .remove(yaml_serde::Value::String("global_scale".into()));
        }
    }
    (
        yaml,
        RegionBindings {
            inputs: vec![
                view(&[2, k], DataType::Fp4, 0),
                view(&[2, k], DataType::Fp4, 4096),
                view(&scales, DataType::Fp8, 8192),
                view(&scales, DataType::Fp8, 16384),
                view(&[1], DataType::Fp32, 24576),
                view(&[1], DataType::Fp32, 28672),
            ],
            outputs: vec![view(&[2, 2], DataType::Fp32, 32768)],
        },
    )
}

#[test]
fn scaled_metadata_round_trips_and_validates_partial_blocks_and_padding() {
    for format in ["nvfp4", "mxfp4"] {
        for layout in ["logical", "padded128x4"] {
            let (yaml, bindings) = fixture(format, layout, 33);
            let spec: ProgramSpec = yaml_serde::from_value(yaml).unwrap();
            validate_program(&spec, &bindings).unwrap();
            let round_trip: ProgramSpec =
                yaml_serde::from_str(&yaml_serde::to_string(&spec).unwrap()).unwrap();
            assert_eq!(spec, round_trip);
        }
    }
}

#[test]
fn native_matrix_rejects_missing_or_wrong_scale_contracts() {
    for issue in [
        "missing_quantization",
        "missing_load",
        "wrong_block",
        "missing_global",
        "wrong_shape",
        "wrong_dtype",
        "wrong_instruction",
        "mixed_formats",
    ] {
        let (mut yaml, mut bindings) = fixture("nvfp4", "logical", 33);
        let mac = &mut yaml["assignments"][0]["expr"]["matrix_mac"];
        match issue {
            "missing_quantization" => {
                mac.as_mapping_mut()
                    .unwrap()
                    .remove(yaml_serde::Value::String("quantization".into()));
            }
            "missing_load" => {
                yaml["metadata_inputs"] = yaml_serde::from_str("[SB, GA, GB]").unwrap();
            }
            "wrong_block" => mac["quantization"][0]["block_size"] = 32.into(),
            "missing_global" => {
                mac["quantization"][0]
                    .as_mapping_mut()
                    .unwrap()
                    .remove(yaml_serde::Value::String("global_scale".into()));
            }
            "wrong_shape" => {
                bindings.inputs[2] =
                    TensorView::new_full(Tensor::new(&[2, 2], &DataType::Fp8, 8192).unwrap());
            }
            "wrong_dtype" => {
                bindings.inputs[2] =
                    TensorView::new_full(Tensor::new(&[2, 3], &DataType::Fp32, 8192).unwrap());
            }
            "wrong_instruction" => mac["execution"]["instruction"] = "tensor".into(),
            "mixed_formats" => mac["quantization"][1]["format"] = "mxfp4".into(),
            _ => unreachable!(),
        }
        let error = yaml_serde::from_value::<ProgramSpec>(yaml)
            .and_then(|spec| validate_program(&spec, &bindings).map_err(serde::de::Error::custom));
        assert!(error.is_err(), "{issue} unexpectedly accepted");
    }
}
