// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

use std::env;
use std::process::Command;

pub const PERFETTO_REPO_URL: &str = "https://github.com/google/perfetto";
pub const PERFETTO_REPO_REFSPEC: &str = "v52.0";

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();

    let output = Command::new("git")
        .arg("init")
        .arg(&out_dir)
        .output()
        .expect("git command failed to start");
    assert!(
        output.status.success(),
        "Failed to init repo for Perfetto source:\n{}",
        str::from_utf8(&output.stderr).unwrap_or_default()
    );

    let output = Command::new("git")
        .args(["-C", &out_dir])
        .arg("fetch")
        .args(["--depth", "1"])
        .arg(PERFETTO_REPO_URL)
        .arg(PERFETTO_REPO_REFSPEC)
        .output()
        .expect("git command failed to start");
    assert!(
        output.status.success(),
        "Failed to fetch Perfetto source repo:\n{}",
        str::from_utf8(&output.stderr).unwrap_or_default()
    );

    let output = Command::new("git")
        .args(["-C", &out_dir])
        .arg("checkout")
        .arg("FETCH_HEAD")
        .output()
        .expect("git command failed to start");
    assert!(
        output.status.success(),
        "Failed to checkout Perfetto source repo:\n{}",
        str::from_utf8(&output.stderr).unwrap_or_default()
    );
}
