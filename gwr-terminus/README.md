<!-- Copyright (c) 2025 Graphcore Ltd. All rights reserved. -->

# gwr-terminus

Terminus captures, runs and converts recipes: YAML files containing useful
commands and their arguments. The package is named `gwr-terminus`; its binary is
named `terminus`. It supports an interactive terminal interface and explicit
command-line operations.

## Before running

Run commands from the GWR workspace root (the directory containing the top-level
`Cargo.toml`). In an Omoikane checkout this is `gwr/`, alongside `workspace/`.
Initialize submodules and install the build dependencies in the
[root README](../README.md#using-gwr-packages). Rustup selects the pinned Rust
toolchain. Development tools and nightly formatting are covered by
[Developing GWR Packages](../README.md#developing-gwr-packages).

## Inspect and run a recipe

```sh
cargo run -p gwr-terminus --bin terminus -- --help
cargo run -p gwr-terminus --bin terminus -- run --recipe examples/sim-ring/recipes/explore_ring_priorities.yaml --recipe-help
```

The second command prints the ring exploration recipe's parameters without
executing its simulations. To execute that recipe, omit `--recipe-help`:

```sh
cargo run -p gwr-terminus --bin terminus -- run --recipe examples/sim-ring/recipes/explore_ring_priorities.yaml
```

Recipes execute their declared commands, which can build binaries, run several
simulations, or write files. Read the YAML and its recipe help to understand the
work and output paths. Running `run` without `--recipe` opens the recipe picker
in an interactive terminal. The default search directory is
`gwr-terminus/recipes`; explicit recipe paths can point elsewhere.

## Capture and export commands

```sh
cargo run -p gwr-terminus --bin terminus -- write --help
cargo run -p gwr-terminus --bin terminus -- convert --recipe gwr-terminus/recipes/fmt.yaml --out /tmp/gwr-format.sh --format bash
```

`write` builds a recipe from shell history and supports selecting commands in a
TUI or supplying explicit selection arguments. `convert` writes a script; the
command above creates `/tmp/gwr-format.sh` without executing its formatting
command. Python is the other supported export format.

Recipe execution stops after a command fails by default. Global
`--no-exit-on-error` changes this behavior. `--keep-tmp` retains temporary files
for diagnosis; `--tmp-file-root` selects their filename prefix.

## Examples and checks

- [Ring priority sweep](../examples/sim-ring/recipes/explore_ring_priorities.yaml).
- [Fabric model sweep](../examples/sim-fabric/recipes/explore_model.yaml).
- [Coverage recipe](../gwr-code-coverage/README.md).
- [Formatting recipe](recipes/fmt.yaml), which modifies source formatting if
  run.

```sh
cargo test -p gwr-terminus
```

The CLI contract is defined in [src/bin/terminus.rs](src/bin/terminus.rs), and
recipe parsing/execution in [src/recipe](src/recipe).

## Recipe verification

Commands in this document are source-reviewed. They have not been smoke-run as
part of this documentation adoption; build, GUI and workload outcomes remain
unverified. Historical observations above retain their original scope.
