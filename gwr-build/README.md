<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-build

Shared helper functions for GWR `build.rs` scripts.

Library crates use `gwr-build` when their crate-level rustdoc should be the same
document as their README. A crate that follows this convention keeps its
overview, examples, and package summary in `README.md`, runs
`write_expanded_readme_docs()` from `build.rs`, and includes the generated
markdown from its top-level `lib.rs`:

```rust,ignore
#![doc = include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]
```

The crate must depend on `gwr-build` from `[build-dependencies]` so the build
script can resolve the helper. The `lib.rs` attribute includes the generated
markdown directly from Cargo's `OUT_DIR`. During generation, repository-relative
links to crate READMEs, Rust source files, and Developer Guide source pages are
rewritten to targets that work from the rendered rustdoc page.

`gwr-build` itself is the bootstrap crate for this convention, so it keeps its
crate-level documentation local to `src/lib.rs`.
