// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus};

fn main() {
    let output_name = "CHANGELOG.md";

    let exit_status = Command::new("git-cliff")
        .args(["--output", output_name])
        .status()
        .unwrap();

    assert_eq!(exit_status, ExitStatus::from_raw(0));

    let exit_status = Command::new("git")
        .args([
            "commit",
            "--amend",
            "--no-edit",
            "--include",
            output_name,
            "--no-verify",
        ])
        .env("SKIP", "git-cliff")
        .status()
        .unwrap();

    assert_eq!(exit_status, ExitStatus::from_raw(0));
}
