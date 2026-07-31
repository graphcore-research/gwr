// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::OnceLock;

use tempfile::TempDir;

static EXPANDED_DOCUMENTATION: OnceLock<String> = OnceLock::new();

pub struct BuilderRun {
    pub output: Output,
    pub documentation: String,
}

impl BuilderRun {
    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.output.stdout).into_owned()
    }

    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }

    pub fn assert_success(&self) {
        assert!(
            self.output.status.success(),
            "adoc-builder failed\nstdout:\n{}\nstderr:\n{}",
            self.stdout(),
            self.stderr()
        );
    }
}

fn expanded_documentation() -> &'static str {
    EXPANDED_DOCUMENTATION.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("gwr-doc-builder must be in the workspace root");
        let target_dir = TempDir::new().expect("failed to create cargo-expand target directory");
        let output = Command::new(env!("CARGO"))
            .current_dir(workspace)
            .env("GWR_DOC_BUILDER", "true")
            .env("CARGO_TARGET_DIR", target_dir.path())
            .args([
                "expand",
                "-p",
                "gwr-docpp",
                "--all-features",
                "--test",
                "documentation-fixture",
            ])
            .output()
            .expect("failed to run cargo expand");

        assert!(
            output.status.success(),
            "cargo expand failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("cargo expand output must be UTF-8")
    })
}

pub fn run_builder(top: &str, extra_args: &[&str]) -> BuilderRun {
    let temp_dir = TempDir::new().expect("failed to create builder test directory");
    let input_path = temp_dir.path().join("expanded.rs");
    let output_path = temp_dir.path().join("documentation.adoc");
    fs::write(&input_path, expanded_documentation()).expect("failed to write expanded Rust");

    let output = Command::new(env!("CARGO_BIN_EXE_adoc-builder"))
        .arg("--input-file")
        .arg(&input_path)
        .arg("--output-file")
        .arg(&output_path)
        .arg("--top")
        .arg(top)
        .args(extra_args)
        .output()
        .expect("failed to run adoc-builder");
    let documentation = fs::read_to_string(output_path).unwrap_or_default();

    BuilderRun {
        output,
        documentation,
    }
}
