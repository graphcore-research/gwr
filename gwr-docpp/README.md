<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-docpp

Procedural macros for embedding generated or externally authored content in Rust
documentation. Use these when ordinary rustdoc Markdown needs included files,
command output, generated sections, or additional markup. The companion
[gwr-doc-builder](../gwr-doc-builder/README.md) can assemble structured AsciiDoc
from expanded source.

| Macro          | Result and requirements                                                         |
| -------------- | ------------------------------------------------------------------------------- |
| `include_str!` | Includes a UTF-8 file using a path relative to the workspace root.              |
| `cmd!`         | Captures command output during compilation. The complete command is one string. |
| `section!`     | Generates a Markdown heading and body from `title` and `text` strings.          |
| `toc!`         | Declares a document/chapter/section table of contents.                          |
| `adoc!`        | Embeds rendered HTML; needs the `asciidoctor` Cargo feature and executable.     |
| `typst!`       | Embeds rendered SVG; needs the `typst` Cargo feature and executable.            |

The default feature set is empty. Commands and external renderers execute while
Rust is compiling the documentation macros, so their inputs and tools are part
of the documentation build.

## Before running

Run commands from the GWR workspace root (the directory containing the top-level
`Cargo.toml`). In an Omoikane checkout this is `gwr/`, alongside `workspace/`.
Initialize submodules and install the build dependencies in the
[root README](../README.md#using-gwr-packages). Rustup selects the pinned Rust
toolchain. Development tools and nightly formatting are covered by
[Developing GWR Packages](../README.md#developing-gwr-packages).

## Use

```rust
#[doc = gwr_docpp::section!(title = "Overview", text = "A documented function.")]
pub fn example() {}
```

To include an existing workspace file:

```rust
#[doc = gwr_docpp::include_str!("gwr-docpp/tests/fixtures/included.txt")]
pub fn included_example() {}
```

Unlike Rust's built-in `include_str!`, this path starts at the workspace root.
[tests/fixtures/documentation.rs](tests/fixtures/documentation.rs) shows the
full TOC syntax, nested sections, command output and optional markup together.

## Check and generate documentation

```sh
cargo test -p gwr-docpp
cargo doc -p gwr-docpp --no-deps --open
```

The default tests exercise command capture, included text, sections, TOCs and
invalid macro inputs. To exercise both external renderers, first install
`asciidoctor` and `typst` and make them available on `PATH`, then run:

```sh
cargo test -p gwr-docpp --all-features
cargo doc -p gwr-docpp --all-features --no-deps --open
```

The documentation command writes rustdoc output to the configured Cargo target
directory and opens it. AsciiDoc and Typst output are embedded in the resulting
documentation. For an assembled `.adoc` file, follow the
[adoc-builder workflow](../gwr-doc-builder/README.md).

## Recipe verification

Commands in this document are source-reviewed. They have not been smoke-run as
part of this documentation adoption; build, GUI and workload outcomes remain
unverified. Historical observations above retain their original scope.
