<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Advanced Setup

## sccache

[sccache] is a compiler cache that supports Rust and other languages, and its
use helps to decrease the build times of GWR packages under many circumstances.

### Installation

[sccache] can be [installed] using system package manages or via Cargo:

```bash
cargo binstall --disable-telemetry --locked sccache
```

### Configuration

#### Cargo

Cargo must be configured to use [sccache] via the [rustc-wrapper] setting. This
can be set via an [environment variable] or via a [configuration file].

To config Cargo globally to use [sccache] add the following to
`$HOME/.cargo/config.toml`:

```toml
[build]
rustc-wrapper = "/Users/<username>/.cargo/bin/sccache"
```

<!-- prettier-ignore-start -->

> [!NOTE]
> This assumes that [sccache] has been installed with Cargo. Update the path to
> the sccache binary if appropriately if a system package manager has been used.

<!-- prettier-ignore-end -->

#### sccache

By default [sccache] requires absolute path matches to achieve a cache hit. This
can be mitigated by setting [SCCACHE_BASEDIRS] appropriately.

For example, assume that the GWR repo will be checked out in the directory
`Volumes/projects/gwr` and various worktrees will be created as subdirectories
within the `Volumes/projects/gwr-wt` directory. To configure the basedirs such
that multiple checkouts and worktrees of the GWR repo in these directories can
all share a cache the following is required:

```toml
basedirs = ["/Volumes/projects/", "/Volumes/projects/gwr-wt/"]
```

On MacOS this config should be written to
`$HOME/Library/Application\ Support/Mozilla.sccache/config`.

### Status

To see the current cache status run:

```bash
sccache -s
```

### Caveats

#### Users

Care must be taken when to avoid running Cargo commands as different users when
[sccache] is configured. To avoid using [sccache] for certain Cargo commands set
`RUSTC_WRAPPER = ""`, for example, when using `sudo`:

```bash
sudo RUSTC_WRAPPER="" cargo ...
```

#### Agents

Tools such as Codex can run Cargo in environments where [sccache] is not
avaliable resulting it unexpected failures for agents.

To avoid Codex attempting to run Cargo commands with [sccache] the following can
be added to `$HOME/.codex/config.toml`:

```toml
[shell_environment_policy]
set = { RUSTC_WRAPPER = "" }
```

[configuration file]:
  https://doc.rust-lang.org/cargo/reference/config.html#hierarchical-structure
[environment variable]:
  https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-reads
[installed]: https://github.com/mozilla/sccache#installation
[rustc-wrapper]:
  https://doc.rust-lang.org/cargo/reference/config.html#buildrustc-wrapper
[sccache]: https://github.com/mozilla/sccache
[SCCACHE_BASEDIRS]:
  https://github.com/mozilla/sccache#normalizing-paths-with-sccache_basedirs
