<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Flaky component

A minimal custom component that randomly drops objects between a source and a
sink. Start here to learn component registration, typed ports, and running the
single-threaded simulation engine.

## Before running

Run commands from the GWR workspace root, the directory containing the top-level
`Cargo.toml`. In Omoikane this is `gwr/`, alongside `workspace/`. Initialize the
repository submodules and install the build dependencies described in the
[root README](../../README.md#using-gwr-packages). Rustup selects the pinned
Rust toolchain automatically.

## Run

```sh
cargo run --release --bin flaky-component -- --seed 1 --drop 0.5 --num-packets 1000
```

The program prints `Sink received N/1000`. The drop ratio is a probability; `N`
need not be exactly 500. Use the same seed when comparing changes. Set
`--drop 0` to pass all objects, or `--drop 1` to drop all objects. Keep the
ratio within `[0, 1]`.

```sh
cargo run --release --bin flaky-component -- --help
cargo test -p flaky-component
```

This example prints its result to the terminal; it does not create a trace or
report file.

## Read and change the model

- [src/main.rs](src/main.rs) creates the source, component and sink, connects
  their ports, and prints the sink count.
- [src/lib.rs](src/lib.rs) implements `Flaky` and its asynchronous `run()` task.
- [tests/flaky.rs](tests/flaky.rs) connects the component into a small
  testbench.

Try changing `--drop` while keeping the seed and packet count fixed. Then see
[flaky-with-delay](../flaky-with-delay/README.md) for composition with a delay
component, or the
[component-writing guide](../../gwr-developer-guide/md_src/components/writing_a_component.md).

## Recipe verification

Commands in this document are source-reviewed. They have not been smoke-run as
part of this documentation adoption; build, GUI and workload outcomes remain
unverified. Historical observations above retain their original scope.
