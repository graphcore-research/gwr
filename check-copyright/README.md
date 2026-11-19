<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Copyright notice checker

A command-line check used by the repository's `prek` hooks. It reads the files
passed as positional arguments and reports missing Graphcore copyright notices.
It checks text; it does not add or modify notices.

## Before running

Run commands from the GWR workspace root (the directory containing the top-level
`Cargo.toml`). In an Omoikane checkout this is `gwr/`, alongside `workspace/`.
Initialize submodules and install the build dependencies in the
[root README](../README.md#using-gwr-packages). Rustup selects the pinned Rust
toolchain. Development tools and nightly formatting are covered by
[Developing GWR Packages](../README.md#developing-gwr-packages).

## Run

```sh
cargo run -p check-copyright -- README.md LICENSE
```

Success exits with status zero and no checker output. A file with no matching
notice produces `No valid copyright notice found in ...` and exit status 1.
Unreadable files return an I/O error. At least one file path is required.

```sh
cargo run -p check-copyright -- --help
cargo test -p check-copyright
```

## What is accepted

For ordinary files the checker looks for:

```text
Copyright (c) 2026 Graphcore Ltd. All rights reserved.
```

Any single year from 2020 through 2029 matches the current regular expression. A
file whose basename is exactly `LICENSE` uses the separate pattern
`Copyright (c) 202N Graphcore Ltd.` followed by a newline. The checker does not
require a particular comment syntax or a notice at the start of the file.

The hook in [prek.toml](../prek.toml) supplies file selection and exclusions;
the binary itself checks every path it receives and does not recursively scan
directories. See [src/main.rs](src/main.rs) for the exact regular expressions.

## Recipe verification

Commands in this document are source-reviewed. They have not been smoke-run as
part of this documentation adoption; build, GUI and workload outcomes remain
unverified. Historical observations above retain their original scope.
