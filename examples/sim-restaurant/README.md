<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Restaurant staffing simulation

Use GWR beyond silicon: customers, queues, till workers and kitchen workers are
modeled with asynchronous tasks and delays. The command-line application sweeps
staffing combinations and ranks their simulated profitability. A separate TUI
inspects one configuration.

## Before running

Run commands from the GWR workspace root, the directory containing the top-level
`Cargo.toml`. In Omoikane this is `gwr/`, alongside `workspace/`. Initialize the
repository submodules and install the build dependencies described in the
[root README](../../README.md#using-gwr-packages). Rustup selects the pinned
Rust toolchain automatically.

## Run a staffing sweep

```sh
cargo run --release --bin sim-restaurant -- --min-till-staff 1 --max-till-staff 2 --min-kitchen-staff 1 --max-kitchen-staff 4 --top-results 4
```

Output starts with a restaurant demand plan, then prints a table containing
staffing, served/balked/gave-up counts, revenue, costs, profit, finish time and
maximum queues, followed by the best configuration summary. These results are
for the model's configured demand and service assumptions.

## Inspect one configuration

```sh
cargo run --release --bin sim-restaurant-tui -- --till-staff 2 --kitchen-staff 5
```

Run the TUI in an interactive terminal. Use each binary's `--help` for its
simulation and display options:

```sh
cargo run --release --bin sim-restaurant -- --help
cargo run --release --bin sim-restaurant-tui -- --help
cargo test -p sim-restaurant
```

The ordinary sweep prints to the terminal. Tracking options are available on the
CLI, but trace output requires exactly one staffing configuration: set each
minimum equal to its corresponding maximum. File output is controlled by the
tracker flags shown in help.

## Source and next steps

[config.rs](src/config.rs) defines demand and service settings;
[sim.rs](src/sim.rs) runs the model and computes summaries. The CLI binaries are
in [src/bin](src/bin), with the interactive interface in [src/tui](src/tui).
Change one staffing or demand parameter at a time. The
[Beyond Silicon guide](../../gwr-developer-guide/md_src/applications/restaurant.md)
explains how the restaurant concepts map to GWR.

## Recipe verification

Commands in this document are source-reviewed. They have not been smoke-run as
part of this documentation adoption; build, GUI and workload outcomes remain
unverified. Historical observations above retain their original scope.
