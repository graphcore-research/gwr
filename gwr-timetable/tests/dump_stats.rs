// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;
use std::process::Command;

use tempfile::tempdir;

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
    // Both operands request the same cold line concurrently. Joining its
    // pending fill is a miss, rather than a hit on already resident data.
    assert!(
        stdout.contains("Hits: 0, misses: 2, hit rate: 0.00%"),
        "stdout did not contain cache hit/miss stats:\n{stdout}"
    );
    assert!(
        stdout.contains("Read: 32 bytes"),
        "coalesced cold reads should issue one backing read:\n{stdout}"
    );
}

#[test]
fn dump_stats_counts_physical_tensor_view_bytes() {
    let temp = tempdir().unwrap();
    let timetable = temp.path().join("timetable.yaml");
    fs::write(
        &timetable,
        r"
nodes:
  - id: input0
    kind: tensor
    config: { addr: 0x100000000, dtype: int4, shape: [4] }
  - id: input1
    kind: tensor
    config: { addr: 0x100000010, dtype: int4, shape: [4] }
  - id: add
    kind: compute
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
    pe: pe0
    input_views:
      - { offsets: [1], shape: [2] }
      - { offsets: [1], shape: [2] }
    output_views:
      - { offsets: [1], shape: [2] }
  - id: output
    kind: tensor
    config: { addr: 0x100000020, dtype: int4, shape: [4] }
edges:
  - { from: input0, to: add.0, kind: data }
  - { from: input1, to: add.1, kind: data }
  - { from: add, to: output, kind: data }
",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gwr-timetable"))
        .arg("--platform")
        .arg("../gwr-platform/examples/simple_pe_cache_mem.yaml")
        .arg("--timetable")
        .arg(&timetable)
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
    assert!(
        stdout.contains("loads 4 bytes, stores 2 bytes"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("Data: 4 bytes consumed, 2 bytes produced"),
        "stdout:\n{stdout}"
    );
}

#[test]
fn paired_regions_report_actual_intermediate_traffic_and_timing() {
    let run = |name: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_gwr-timetable"))
            .arg("--platform")
            .arg("examples/compute_region_platform.yaml")
            .arg("--timetable")
            .arg(format!("examples/{name}_region.yaml"))
            .arg("--stdout")
            .arg("--dump-stats")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let fused = run("fused");
    let materialized = run("materialized");
    assert!(fused.contains("Time now 33ns"), "{fused}");
    assert!(materialized.contains("Time now 58ns"), "{materialized}");
    assert!(
        fused.contains("planned loads 96 bytes, stores 32 bytes"),
        "{fused}"
    );
    assert!(
        materialized.contains("planned loads 128 bytes, stores 64 bytes"),
        "{materialized}"
    );
    assert!(
        fused.contains("actual loads 96 bytes, stores 32 bytes"),
        "{fused}"
    );
    assert!(
        materialized.contains("actual loads 128 bytes, stores 64 bytes"),
        "{materialized}"
    );
    assert!(
        fused.contains("machine ops {\"add\": 8, \"mul\": 8}"),
        "{fused}"
    );
    assert!(
        materialized.contains("machine ops {\"add\": 8, \"mul\": 8}"),
        "{materialized}"
    );
}
