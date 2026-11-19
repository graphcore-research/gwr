<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Scrambler

A four-port component with two internal stores. Its connection methods either
preserve or swap the paths between its two inputs and two outputs. This shows
how a component can expose ports belonging to subcomponents without adding a
custom asynchronous task of its own.

## Before running

Run commands from the GWR workspace root, the directory containing the top-level
`Cargo.toml`. In Omoikane this is `gwr/`, alongside `workspace/`. Initialize the
repository submodules and install the build dependencies described in the
[root README](../../README.md#using-gwr-packages). Rustup selects the pinned
Rust toolchain automatically.

## Run

```sh
cargo run --release --bin scrambler
cargo run --release --bin scrambler -- --scramble
```

The first command prints `Input order: 1, 2`; the second prints
`Input order: 2, 1`. These values are the counts at the two sinks: the sources
send one and two objects respectively. The example does not write output files.

```sh
cargo run --release --bin scrambler -- --help
cargo test -p scrambler
```

## Read the component

[src/main.rs](src/main.rs) connects two sources and two sinks.
[src/lib.rs](src/lib.rs) contains the connection methods and the internal
`Store` components. `--scramble` changes these connections during construction;
it does not rewire a running simulation. The component derives `Runnable`
because its stores do the asynchronous work.

## Recipe verification

Commands in this document are source-reviewed. They have not been smoke-run as
part of this documentation adoption; build, GUI and workload outcomes remain
unverified. Historical observations above retain their original scope.
