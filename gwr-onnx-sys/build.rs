// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs::read_dir;
use std::process::Command;

#[path = "src/submodule_path.rs"]
mod submodule_path;

const SEMVER_ENV_VAR_KEY: &str = "CARGO_SEMVER_CHECKS";

fn main() {
    println!("cargo::rerun-if-changed={}", submodule_path::ONNX_SOURCE);

    if cargo_semver_checks_enabled() && !submodule_is_updated() {
        update_submodule();
    }

    assert!(
        submodule_is_updated(),
        "{}",
        &format!(
            "ONNX source submodule does not appear to have been checked out. Please run `git {}`.",
            get_git_args().join(" ")
        )
    );
}

fn cargo_semver_checks_enabled() -> bool {
    let cargo_semver_checks = std::env::var(SEMVER_ENV_VAR_KEY)
        .unwrap_or_default()
        .parse()
        .unwrap_or(0);

    cargo_semver_checks == 1
}

fn submodule_is_updated() -> bool {
    let Ok(mut directory_entries) = read_dir(submodule_path::ONNX_SOURCE) else {
        return false;
    };

    directory_entries.next().is_some()
}

fn update_submodule() {
    let Ok(git_command) = Command::new("git").args(get_git_args()).status() else {
        panic!("git should be available on the PATH when {SEMVER_ENV_VAR_KEY} is set.")
    };
    assert!(git_command.success(), "{}", git_command.to_string());
}

fn get_git_args() -> [&'static str; 5] {
    [
        "submodule",
        "update",
        "--init",
        "--recommend-shallow",
        submodule_path::ONNX_SOURCE,
    ]
}
