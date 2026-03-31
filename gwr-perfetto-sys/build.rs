// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;
use std::process::Command;

#[path = "src/submodule_path.rs"]
mod submodule_path;

const SEMVER_ENV_VAR_KEY: &str = "CARGO_SEMVER_CHECKS";

fn main() {
    println!(
        "cargo::rerun-if-changed={}",
        submodule_path::PERFETTO_SOURCE
    );

    if cargo_semver_checks_enabled() && !submodule_is_updated() {
        update_submodule();
    }

    assert!(
        submodule_is_updated(),
        "{}",
        &format!(
            "Perfetto source submodule does not appear to have been checked out.
    Please run `git {}`.",
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
    let mut directory_entries = fs::read_dir(submodule_path::PERFETTO_SOURCE).expect(
        "The Perfetto source
    directory is expected to exist whether or not the submodule has been
    initialised and updated.",
    );

    directory_entries.next().is_some()
}

fn update_submodule() {
    let git_command = Command::new("git")
        .args(get_git_args())
        .status()
        .unwrap_or_else(|_| {
            panic!("git should be available on the PATH when {SEMVER_ENV_VAR_KEY} is set.")
        });
    assert!(git_command.success(), "{}", git_command.to_string());
}

fn get_git_args() -> [&'static str; 4] {
    [
        "submodule",
        "update",
        "--init",
        submodule_path::PERFETTO_SOURCE,
    ]
}
