<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-doc-builder

Extract structured AsciiDoc documentation from expanded Rust source. The library
parses documentation attributes and a table of contents; the `adoc-builder`
binary writes the assembled document. Run `cargo expand` first so procedural
macros and documentation attributes have been expanded.

This package assembles AsciiDoc source. Rendering it as HTML or PDF is a
separate Asciidoctor step. For ordinary Rust API documentation use
`cargo doc-gwr`; for the Developer Guide use mdBook.

## Before running

Run commands from the GWR workspace root (the directory containing the top-level
`Cargo.toml`). In an Omoikane checkout this is `gwr/`, alongside `workspace/`.
Initialize submodules and install the build dependencies in the
[root README](../README.md#using-gwr-packages). Rustup selects the pinned Rust
toolchain. Development tools and nightly formatting are covered by
[Developing GWR Packages](../README.md#developing-gwr-packages).

## Build an included documentation fixture

Install `cargo-expand` with the development dependencies. Run the following from
the GWR workspace root; paths in embedded includes are workspace-relative.

```sh
GWR_DOC_BUILDER=true cargo expand -p gwr-docpp --all-features --test documentation-fixture > /tmp/gwr-documentation-expanded.rs
cargo run -p gwr-doc-builder --bin adoc-builder -- --input-file /tmp/gwr-documentation-expanded.rs --output-file /tmp/gwr-documentation.adoc --top crate
```

This is the same input and top-level path used by the
[integration tests](tests/common/mod.rs). It writes expanded Rust and an
AsciiDoc document under `/tmp`. `GWR_DOC_BUILDER=true` preserves documentation
markers during expansion. The fixture deliberately references `crate::missing`,
so a missing-module warning is expected; the golden output is retained in
[tests/fixtures/expected.adoc](tests/fixtures/expected.adoc).

`--top` is required. It names the parsed block containing the table of contents,
such as `crate`; an unknown path or a block without a TOC is an error. Use
`--dump-all` to print parsed blocks and `--verbose` for additional diagnostics.

```sh
cargo run -p gwr-doc-builder --bin adoc-builder -- --help
cargo test -p gwr-doc-builder
```

The integration tests invoke `cargo expand` internally. The companion
[gwr-docpp](../gwr-docpp/README.md) package supplies `toc!`, `section!`,
`include_str!` and the optional markup macros used by the fixture.

## Source

[src/bin/adoc-builder.rs](src/bin/adoc-builder.rs) defines the CLI;
[src/doc_parser.rs](src/doc_parser.rs) parses and assembles the document;
[src/toc.rs](src/toc.rs) defines the TOC format.

## Recipe verification

Commands in this document are source-reviewed. They have not been smoke-run as
part of this documentation adoption; build, GUI and workload outcomes remain
unverified. Historical observations above retain their original scope.
