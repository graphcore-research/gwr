<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Flow-controlled pipeline

Explore how credit return delay, data delay and buffer capacity affect a
pipeline's throughput. The model connects a traffic source, rate limiter,
`FcPipeline` and sink, and checks that all expected frames arrive.

## Before running

Run commands from the GWR workspace root, the directory containing the top-level
`Cargo.toml`. In Omoikane this is `gwr/`, alongside `workspace/`. Initialize the
repository submodules and install the build dependencies described in the
[root README](../../README.md#using-gwr-packages). Rustup selects the pinned
Rust toolchain automatically.

## Run

```sh
cargo run --release --bin sim-pipe -- --stdout --pipe-data-delay 10 --pipe-buffer-entries 10 --pipe-credit-delay 10
```

With stdout tracking enabled, successful completion includes `Pass: Sent ...`
and payload/total throughput summaries. Modeled elapsed time is reported in
nanoseconds. These are simulation results, not measured hardware throughput. An
incomplete transfer reports a deadlock error.

Change `--pipe-buffer-entries` while keeping both delays fixed, then change one
of the delays. `--bytes-to-send` controls the workload size. `--progress` shows
a progress bar; `--help` lists traffic, clock termination, and tracker options.

```sh
cargo run --release --bin sim-pipe -- --help
cargo test -p sim-pipe
```

The command above reports to stdout. Trace output is optional and is controlled
by the tracker flags listed in help. A nonzero `--finish-tick` deliberately
terminates the simulation at that tick; it is not a completion criterion.

## Source

[src/main.rs](src/main.rs) constructs the model, validates delivery, and
computes the summaries. [src/frame_gen.rs](src/frame_gen.rs) generates traffic.
The pipeline model is in
[gwr-models](../../gwr-models/README.md#flow-controlled-pipeline).

## Recipe verification

Commands in this document are source-reviewed. They have not been smoke-run as
part of this documentation adoption; build, GUI and workload outcomes remain
unverified. Historical observations above retain their original scope.
