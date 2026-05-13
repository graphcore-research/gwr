// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;
use std::path::Path;

use gwr_onnx_sys::ONNX_SOURCE;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = fs::canonicalize(Path::new(ONNX_SOURCE)).unwrap();

    println!(
        "cargo:rerun-if-changed={}/onnx/onnx.proto",
        proto_root.display()
    );
    println!(
        "cargo:rerun-if-changed={}/onnx/onnx-operators.proto",
        proto_root.display()
    );

    let mut prost_build = prost_build::Config::new();
    prost_build.compile_protos(
        &[proto_root.join("onnx/onnx-operators.proto")],
        &[proto_root],
    )?;

    Ok(())
}
