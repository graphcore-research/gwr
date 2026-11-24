<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-config

<!-- ANCHOR: configuration -->

The `gwr_config` library provides a hierarchical configuration mechanism for
applications which wish to accept settings and options from a TOML configuration
file, environment variables, and a command-line interface. The configuration
file path comes from the macro-level default, or from a generated command-line
flag when the macro enables one.

Explicit command-line arguments take precedence over `GWR_` environment
variables, then TOML values, then typed Rust defaults. Custom Clap parsers apply
only to command-line arguments. TOML and environment values use Serde
deserialization. To enforce constraints regardless of where a value comes from,
validate the final configuration, including defaults.

A **defaultable leaf configuration** is a configuration struct with no flattened
child configurations, where every field is optional (`Option<T>`) or has an
explicit `default_value_t` default.

These configurations also support direct Clap parsing. This applies typed
defaults without loading environment variables or files. Defaults need no
`Display` implementation and do not need a command-line representation. Raw
`ArgMatches` stay sparse. Non-enum defaults require `Debug` for their help
descriptions. Direct leaf configurations preserve omitted fields.

<!-- ANCHOR_END: configuration -->

This is an application-focused utility library, not a general Clap/Figment
integration. It supports simple typed defaults, scalar/optional/vector fields,
explicit boolean flags, custom CLI parsers, paired flat configuration structs,
and leaf exclusive groups. Cross-field and domain validation belongs in the
application. Unsupported Clap and Serde attributes are rejected during macro
expansion.

See `multi_source_config`'s API documentation for the complete supported subset,
source/default semantics, file selection, and intentionally unsupported
features. The `application_subset` example and tests exercise this contract.

## Debug

The macro attempts to detect unsupported use cases, panicking with a message
that should help to identify the issue quickly.

For cases where this is not possible, the `macro-backtrace` feature and the
`cargo-expand` plugin can be helpful in understanding what is going wrong:

- `RUSTFLAGS="-Zmacro-backtrace" cargo +nightly run -p gwr-track --example log_test`
- `cargo expand -p gwr-track --example log_test`
