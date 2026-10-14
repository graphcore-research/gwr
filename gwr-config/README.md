<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-config

The `gwr_config` library provides a hierarchical configuration mechanism for
applications which wish to accept settings and options from a TOML configuration
file, environment variables, and a command-line interface. The configuration
file path comes from the macro-level default, or from a generated command-line
flag when the macro enables one.

This is an application-focused adapter, not a general Clap/Figment integration.
It supports simple typed defaults, scalar/optional/vector fields, explicit
boolean flags, custom CLI parsers, paired flat configuration structs, and leaf
exclusive groups. Cross-field and domain validation belongs in the application.
Unsupported Clap and Serde attributes are rejected during macro expansion.

See `multi_source_config`'s API documentation for the complete supported subset,
source/default semantics, file selection, and intentionally unsupported
features. The `application_subset` example and tests exercise this contract.
