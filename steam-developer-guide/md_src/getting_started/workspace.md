<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Workspace

All of the Rust packages that make up the STEAM project are contained within a
single Cargo Workspace. This allows build and test [commands] to be run across
all packages together, as well as for ensuring packages share common versions of
dependencies.

The toplevel `Cargo.toml` controls the workspace, and lists all included
packages in the `workspace.members` field.

{{#include ../links_depth1.md}}
