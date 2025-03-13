<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# STEAM

Welcome to the STEAM (Simulation Technology for Evaluation and Architecture
Modelling) project.

<!-- ANCHOR: tooling_bootstrap -->

## Rust Tools

A Rust toolchain is required to build both models and documentation.

Rust toolchains can be [installed] and managed using the [`rustup`] tool. To
install rustup on macOS:

```bash
brew install rustup
```

It may also be necessary to add rustup to the PATH, as indicated by `brew`.

Or alternatively, rustup can be installed directly with:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Once installed, it can be used to install the required Rust toolchain(s).

[installed]: https://www.rust-lang.org/tools/install
[`rustup`]: https://rust-lang.github.io/rustup/

<!-- ANCHOR_END: tooling_bootstrap -->

<!-- ANCHOR: package_users -->

## Using STEAM packages

For users of the STEAM packages the `default` profile of the `stable` toolchain
is required:

```bash
rustup toolchain install --profile default stable
```

Finally a small number of additional tools must be installed outside of the Rust
ecosystem:

- [Cap'n Proto]

To install these tools on macOS:

```bash
brew install capnp
```

[Cap'n Proto]: https://capnproto.org/

<!-- ANCHOR_END: package_users -->

<!-- ANCHOR: package_developers -->

## Developing STEAM Packages

For developers of the STEAM packages the `default` profile of both `stable` and
`nightly` toolchains are required:

```bash
rustup toolchain install --profile default stable nightly
```

Once the Rust toolchain is installed it can be used to install the rest of the
dependencies required to develop STEAM packages:

```bash
cargo install cargo-deny cargo-expand cargo-semver-checks cocogitto release-plz
```

Finally a small number of additional tools must be installed outside of the Rust
ecosystem:

- [Asciidoctor]
- [Cap'n Proto]
- [pre-commit]
- [Prettier]

To install these tools on macOS:

```bash
brew install asciidoctor capnp pre-commit prettier
```

Finally the pre-commit hooks need to be installed within the cloned copy of the
STEAM repo:

```bash
cd steam
pre-commit install
```

### Committing a Change

All commits made to the STEAM repo must follow the [Conventional Commits] 1.0.0
specification. This allows the automatic generation of both a top-level
changelog covering the whole workspace as well as an individual changelog for
each package within the workspace.

When writing a commmit message in this form:

- The set of `type`s that should generally be used can be found in the
  `commit_parsers` array within the `release-plz.toml` configuration file. This
  array also details the sections each type will be included in within the
  changelog.
- The `optional scope` should be used to detail the name of the updated package.
  - If the change applied to multiple, but not all packages, the names should be
    comma seperated. For example:
    ```text
    feat(steam-track,steam-spotter): capnp binary file support
    ```
  - If the change applies to all Rust packages the scope should be
    `all packages`. For example:
    ```text
    doc(all packages): add example use for all public APIs
    ```
  - If the change applies to infrastructure, CI, or general configuration but
    not specifically the Rust source the scope can be omitted. For example:
    ```text
    infra: set default pre-commit hooks to install
    ```

During the commit process a number of different hooks will be invoked by
[pre-commit]:

- The [Cocogitto] tool is used to lint the text of the commit message, ensuring
  that it adheres to the [Conventional Commits] specification.
- All packages within the workspace will be checked for adherence to proper
  semantic versioning using [cargo-semver-checks].
- All dependencies will be checked for vulnerabilities and compatible licensing
  using [cargo-deny].
- Source files will be formatted using `rustfmt`, [Prettier], and built in tools
  from the [pre-commit-hooks] library.
- Rust source will be linted using [clippy].
- Rust source will be compiled using `cargo check`.

### Making a Release

The release process for all packages within the STEAM workspace is handled by
the [Release-plz] tool.

To start the release process, run:

```bash
release-plz release-pr
```

which will bump the version numbers of any updated packages and update the
package CHANGELOG.md files as required. These changes will be automatically
committed and a pull-request opened on Github for the proposed release to be
reviewed.

Once the pull-request has been approved and merged the release process is
completed by running:

```bash
release-plz release
```

which will tag the repo marking the correct commit with the versions for each
updated package, and automatically publishes the updated packages using
`cargo publish` and as Github releases.

[Asciidoctor]: https://asciidoctor.org
[Cap'n Proto]: https://capnproto.org
[cargo-deny]: https://github.com/EmbarkStudios/cargo-deny
[cargo-semver-checks]: https://github.com/obi1kenobi/cargo-semver-checks
[clippy]: https://doc.rust-lang.org/clippy
[Cocogitto]: https://docs.cocogitto.io
[Conventional Commits]: https://www.conventionalcommits.org/en/v1.0.0/
[pre-commit]: https://pre-commit.com
[pre-commit-hooks]: https://github.com/pre-commit/pre-commit-hooks
[Prettier]: https://prettier.io
[Release-plz]: https://release-plz.dev

<!-- ANCHOR_END: package_developers -->

<!-- ANCHOR: dev_docs -->

## Developer Guide

The developer guide is an `mdbook`. In order to produce the document it is
necessary to first install `mdbook`:

```bash
cargo install mdbook mdbook-cmdrun mdbook-keeper mdbook-linkcheck
```

and then build and open the user guide with:

```bash
cd steam-developer-guide/
mdbook build --open
```

If developing the guide then this command will launch a process that continually
monitors the source and regenerates the HTML if it changes (causing the browser
to automatically refresh):

```bash
mdbook serve --open
```

<!-- ANCHOR_END: dev_docs -->

<!-- ANCHOR: api_docs -->

## API Documentation

Documentation within the STEAM source is done using [`rustdoc`] formatting such
that APIs are documented and any code snippets are compiled and run.

This documentation can be generated by running:

<!-- ANCHOR: api_docs_cmd -->

```bash
cargo doc --all-features --no-deps --open
```

<!-- ANCHOR_END: api_docs_cmd -->

[`rustdoc`]: https://doc.rust-lang.org/rustdoc/what-is-rustdoc.html

<!-- ANCHOR_END: api_docs -->
