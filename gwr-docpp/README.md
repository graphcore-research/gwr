<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-docpp

The GWR documentation pre-processor library of macros allow the embedding of
various markup langauges within Rust source files.

[AsciiDoc] and [Typst] markups are currently supported. Due to the dependency on
external tooling both are guarded with opt-in Cargo features.

[AsciiDoc]: https://asciidoc.org
[Typst]: https://typst.app
