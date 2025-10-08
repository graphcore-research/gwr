// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

//! Perfetto's source will be downloaded and a symlink
//! (see the PERFETTO_SYMLINK const) created in the source tree of this package
//! to give users of this package constant paths to tools and schema.
//!
//! Ideally we would have Cargo watch the PERFETTO_SYMLINK itself such that it
//! would be recreate if deleted so that downtramway builds that depend on this
//! package do not fail unexpectedly. Unfortunately this is not possible as
//! there appears to be no way to prevent the (mtime) check performed by Cargo
//! following the symlink. Instead, as a compromise, we explicitly watch the
//! PERFETTO_SYMLINK/protos directory as this is not expected to change once the
//! Perfetto source has been downloaded (i.e. mtime check will be stable after
//! the first rebuild attempt), and it allows us to detect if the symlink is
//! removed.

use std::env;
use std::process::Command;

pub const PERFETTO_REPO_URL: &str = "https://github.com/google/perfetto";
pub const PERFETTO_REPO_REFSPEC: &str = "v52.0";

pub const PERFETTO_SYMLINK: &str = "perfetto";

fn add_file_build_triggers() {
    println!("cargo::rerun-if-changed=src/lib.rs");
}

fn main() {
    add_file_build_triggers();

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

    let output = Command::new("ln")
        .arg("-s")
        .arg("-f")
        .arg("-n")
        .arg(&out_dir)
        .arg(PERFETTO_SYMLINK)
        .output()
        .expect("ln command failed to start");
    assert!(
        output.status.success(),
        "Failed to create symlink to Perfetto source repo:\n{}",
        str::from_utf8(&output.stderr).unwrap_or_default()
    );

    println!("cargo::rerun-if-changed={PERFETTO_SYMLINK}/protos");
}
