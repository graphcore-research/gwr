// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;
use std::path::Path;

use gwr_onnx_sys::ONNX_SOURCE;

fn main() {
    let onnx_src = fs::canonicalize(Path::new(ONNX_SOURCE)).unwrap();

    let mut prost_build = prost_build::Config::new();
    prost_build
        .compile_protos(&[onnx_src.join("onnx/onnx.proto")], &[onnx_src])
        .expect("Protobuf compiler failed to generate Rust source for ONNX support");
}
