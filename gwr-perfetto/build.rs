// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::fs;
use std::path::Path;

use gwr_perfetto_sys::PERFETTO_SOURCE;

fn main() {
    let perfetto_src = fs::canonicalize(Path::new(PERFETTO_SOURCE)).unwrap();

    let mut prost_build = prost_build::Config::new();
    prost_build
        .compile_protos(
            &[perfetto_src.join("protos/perfetto/trace/trace.proto")],
            &[perfetto_src],
        )
        .expect("Protobuf compiler failed to generate Rust source for Perfetto support");
}
