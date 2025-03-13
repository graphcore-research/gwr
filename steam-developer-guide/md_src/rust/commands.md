<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Useful Commands

The [Rust] toolchain provides a number of useful commands that it is worth being
aware of.

**Note:** The following command should all support `--help` for more
information.

## Build

The default way to build STEAM.

```bash
cd steam/
cargo build
```

Or `cargo build --release` to create a release binary.

A quicker version of the command while you are developping is:

```bash
cargo check
```

This runs all the commands needed to compile the code and report errors without
actually producing the binaries.

## Open the Documentation

The STEAM libraries and APIs are documented using [`rustdoc`]. The documentation
can be built and opened with:

{{#include ../../../README.md:api_docs_cmd}}

## Run the Tests

This command runs all the tests, including compiling and running any snippets in
the documentation.

```bash
cargo test
```

## Run the Benchmarks

There are a number of benchmarks that have been written to be able to understand
the impact of changes on the performance of core components within the [engine].

```bash
cargo bench
```

## Formatting the Code

There is no need to manually format your code as the Rust tools provide a tool
for this that keeps all of the codebase in a consistent format.

This is usually be done using the stable toolchain:

```bash
cargo fmt
```

But for developers of the STEAM packages use of the nightly toolchain is
required:

```bash
cargo +nightly fmt
```

## Helper tools

The `clippy` tool provides some static analysis tools that help to highlight
redundant or not rust-like code that should be refactored

```bash
cargo clippy
```

## Expand

In order to see the pre-processed output the `expand` tool can be used. It first
needs to be installed with:

```bash
cargo install cargo-expand
```

and then is run using:

```bash
cargo expand
```

## Flamegraphs

Flamegraphs are helpful to analyse where the application is spending all of its
time. The simple way to use this is to install it:

```bash
cargo install flamegraph
```

And then it can be run against binaries, tests, benchmarks. This is an example
commandline for running it against the `flakey-component` binary with a few
arguments.

```bash
CARGO_PROFILE_RELEASE_DEBUG=true sudo cargo flamegraph --bin flaky-component -- --num-packets 500000
```

**Note:** This must be run with as root when running on macOS.

**Note:** This is usually most useful against the `release` build.

[engine]: ../steam_engine/chapter.md
[Rust]: https://www.rust-lang.org
[`rustdoc`]: https://doc.rust-lang.org/rustdoc/what-is-rustdoc.html
