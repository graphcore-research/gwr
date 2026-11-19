<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Flaky component with delay

This example extends the dropping component with a `Delay` subcomponent. It
shows how to configure and register a composite component and forward its
external ports to internal components.

## Before running

Run commands from the GWR workspace root, the directory containing the top-level
`Cargo.toml`. In Omoikane this is `gwr/`, alongside `workspace/`. Initialize the
repository submodules and install the build dependencies described in the
[root README](../../README.md#using-gwr-packages). Rustup selects the pinned
Rust toolchain automatically.

## Run

```sh
cargo run --release --bin flaky-with-delay -- --seed 1 --drop 0.5 --num-packets 1000 --delay 10
```

The program prints `Sink received N/1000`. The count depends on the seeded drop
decisions; the delay configures modeled clock ticks through the delay component.
The count alone is not a latency measurement. A drop ratio outside `[0, 1]`
prints an error and exits unsuccessfully.

```sh
cargo run --release --bin flaky-with-delay -- --help
cargo test -p flaky-with-delay
```

No trace or report file is created by this command.

## Read and change the model

[src/main.rs](src/main.rs) parses the settings and connects the test system.
[src/lib.rs](src/lib.rs) defines the configuration, creates the delay
subcomponent, and implements the custom run behavior. Compare it with
[flaky-component](../flaky-component/README.md) to see what composition adds.
Change `--delay` or `--drop` one at a time to keep the experiment interpretable.

## Recipe verification

Commands in this document are source-reviewed. They have not been smoke-run as
part of this documentation adoption; build, GUI and workload outcomes remain
unverified. Historical observations above retain their original scope.
