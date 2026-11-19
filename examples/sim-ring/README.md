<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Ring interconnect

A ring of nodes with local traffic injection, rate limiters and
credit-controlled links. Use it to investigate how arbitration priority and
finite buffers can produce deadlock.

## Before running

Run commands from the GWR workspace root, the directory containing the top-level
`Cargo.toml`. In Omoikane this is `gwr/`, alongside `workspace/`. Initialize the
repository submodules and install the build dependencies described in the
[root README](../../README.md#using-gwr-packages). Rustup selects the pinned
Rust toolchain automatically.

## Run

```sh
cargo run --release --bin sim-ring -- --stdout
```

The program checks every sink against its expected frame count. Completion
prints `Pass (...)` with simulated time when stdout tracking is enabled.
Undelivered frames produce a `Deadlock detected` diagnostic and an error; a
deadlock is an intentional outcome to explore, rather than a successful run.

```sh
cargo run --release --bin sim-ring -- --help
cargo run --release --bin terminus -- run --recipe examples/sim-ring/recipes/explore_ring_priorities.yaml --recipe-help
```

The second command prints the supplied sweep recipe's parameters without
executing its simulations. To run that exploration, omit `--recipe-help`. The
recipe and its parameters control ring size, priority, buffers and tracing.
Recipe execution may run multiple simulations.

For direct comparisons, vary `--ring-priority` or buffer sizes one at a time.
`--bytes-to-send` sets bytes per source; `--ring-size` sets the number of nodes.
The basic command writes to stdout; use the tracker flags in help to capture
files. `--progress` adds a progress bar.

## Source and checks

[src/main.rs](src/main.rs) constructs the ring and checks delivery.
[src/ring_builder.rs](src/ring_builder.rs) builds the node/link groups. The
[recipe](recipes/explore_ring_priorities.yaml) defines the exploration.

```sh
cargo test -p sim-ring
```

## Recipe verification

Commands in this document are source-reviewed. They have not been smoke-run as
part of this documentation adoption; build, GUI and workload outcomes remain
unverified. Historical observations above retain their original scope.
