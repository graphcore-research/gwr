// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::process::{Command, Output};

const INPUT_ERROR: &str = "exactly one of `--log` or `--bin` must be provided";

fn run_spotter(log: Option<&str>, bin: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gwr-spotter"));
    command.env_remove("GWR_LOG").env_remove("GWR_BIN");
    if let Some(log) = log {
        command.env("GWR_LOG", log);
    }
    if let Some(bin) = bin {
        command.env("GWR_BIN", bin);
    }
    command.output().unwrap()
}

#[test]
fn rejects_a_missing_input_after_merging_sources() {
    let run = run_spotter(None, None);

    assert!(!run.status.success());
    assert!(
        String::from_utf8_lossy(&run.stderr).contains(INPUT_ERROR),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn rejects_multiple_inputs_after_merging_sources() {
    let run = run_spotter(Some("trace.log"), Some("trace.bin"));

    assert!(!run.status.success());
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
    assert!(stderr.contains("--log"), "{stderr}");
    assert!(stderr.contains("--bin"), "{stderr}");
}

#[cfg(feature = "perfetto")]
#[test]
fn mixed_case_environment_keeps_same_source_conflicts() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gwr-spotter"));
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GWR_")
        {
            command.env_remove(key);
        }
    }
    let run = command
        .arg("--no-server")
        .env("GWR_LOG", "/nonexistent-gwr-review.log")
        .env("GWR_perfetto", "/nonexistent-gwr-review.pftrace")
        .output()
        .unwrap();
    assert!(!run.status.success());
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("`--perfetto` requires `--bin`"), "{stderr}");
}
