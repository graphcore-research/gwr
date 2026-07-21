// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::process::Command;

#[test]
fn dump_stats_includes_cache_stats() {
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-timetable"))
        .arg("--platform")
        .arg("../gwr-platform/examples/simple_pe_cache_mem.yaml")
        .arg("--timetable")
        .arg("examples/cache.yaml")
        .arg("--stdout")
        .arg("--dump-stats")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "gwr-timetable failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Cache totals:"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("Cache top::l1_0:"),
        "stdout did not contain per-cache stats:\n{stdout}"
    );
    assert!(
        stdout.contains("Payload read: 64 bytes"),
        "stdout did not contain cache payload read stats:\n{stdout}"
    );
    assert!(
        stdout.contains("INFO:   Payload read: 64 bytes"),
        "stdout did not contain a prefixed cache payload read stat line:\n{stdout}"
    );
    assert!(
        stdout.contains("Hits: 1, misses: 1, hit rate: 50.00%"),
        "stdout did not contain cache hit/miss stats:\n{stdout}"
    );
}
