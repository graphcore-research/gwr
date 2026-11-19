<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Fabric model comparison

Compare functional and routed implementations of the same rectangular fabric.
This example explores the effect of routing, traffic pattern, buffering and hop
delay on modeled throughput.

## Before running

Run commands from the GWR workspace root, the directory containing the top-level
`Cargo.toml`. In Omoikane this is `gwr/`, alongside `workspace/`. Initialize the
repository submodules and install the build dependencies described in the
[root README](../../README.md#using-gwr-packages). Rustup selects the pinned
Rust toolchain automatically.

## Run

```sh
cargo run --release --bin sim-fabric -- --stdout
cargo run --release --bin sim-fabric -- --stdout --routed
```

The first command uses the functional fabric; the second selects the routed
model. Both check delivered frame counts and report simulated elapsed time and
payload/total throughput on success. An incomplete transfer reports a deadlock.
Different timing results reflect the chosen model and assumptions.

```sh
cargo run --release --bin sim-fabric -- --help
cargo run --release --bin terminus -- run --recipe examples/sim-fabric/recipes/explore_model.yaml --recipe-help
```

The recipe-help command prints the available sweep parameters without running
the simulations. Omit `--recipe-help` to execute it. A second recipe,
[explore_ticks_per_hop.yaml](recipes/explore_ticks_per_hop.yaml), varies hop
delay. Recipe execution can run multiple simulations.

Try `--traffic-pattern all-to-one` on both direct commands. Keep geometry,
workload and seed fixed when comparing the fabric implementations. Other useful
settings include `--fabric-columns`, `--fabric-rows`, `--port-bits-per-tick`,
`--ticks-per-hop`, and `--bytes-to-send`. The basic commands write to stdout;
tracker options in help enable file output.

## Source and checks

[src/main.rs](src/main.rs) selects and connects the fabric and computes the
summaries. [src/access_gen.rs](src/access_gen.rs) defines traffic patterns;
[src/source_sink_builder.rs](src/source_sink_builder.rs) builds endpoints.

```sh
cargo test -p sim-fabric
```

## Recipe verification

Commands in this document are source-reviewed. They have not been smoke-run as
part of this documentation adoption; build, GUI and workload outcomes remain
unverified. Historical observations above retain their original scope.
