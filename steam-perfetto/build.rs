// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::fs;
use std::path::Path;

const STEAM_DOCS_ONLY_ENV_VAR: &str = "STEAM_DOCS_ONLY";
const DOCS_RS_ENV_VAR: &str = "DOCS_RS";

const PERFETTO_SRC_DIR: &str = "../steam-perfetto-sys/perfetto/";

fn add_env_build_triggers() {
    println!("cargo::rerun-if-env-changed={STEAM_DOCS_ONLY_ENV_VAR}");
    println!("cargo::rerun-if-env-changed={DOCS_RS_ENV_VAR}");
}

fn compile_perfetto_schema() {
    let perfetto_src = fs::canonicalize(Path::new(PERFETTO_SRC_DIR)).unwrap();

    let mut prost_build = prost_build::Config::new();
    if std::env::var(STEAM_DOCS_ONLY_ENV_VAR).is_err() && std::env::var(DOCS_RS_ENV_VAR).is_err() {
        // Only allow the use of the Protobuf compiler when a regular build is
        // occuring. Perfetto will not be built during documentation-only
        // builds, regardless of the features enabled.

        #[cfg(feature = "_perfetto-protoc")]
        prost_build.protoc_executable(perfetto_src.join("out/ui/protoc"));
    }
    prost_build
        .compile_protos(
            &[perfetto_src.join("protos/perfetto/trace/trace.proto")],
            &[perfetto_src],
        )
        .expect("Protobuf compiler failed to generate Rust source for Perfetto support");
}

fn main() {
    add_env_build_triggers();

    compile_perfetto_schema();
}
