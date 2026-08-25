// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

#![doc(test(attr(deny(unused_must_use))))]
#![doc = std::include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]

/// Auto-generated Perfetto Trace module
///
/// The contents of this file are created by `build.rs` at compile-time. They
/// provide all the functions required to build up
/// [Perfetto Trace Packets](https://perfetto.dev/docs/reference/synthetic-track-event)
/// containing
/// [Perfetto TrackEvents](https://perfetto.dev/docs/instrumentation/track-events).
pub mod protos {
    // No need to emit warnings for auto-generated Protobuf code
    #![allow(missing_docs)]
    #![allow(rustdoc::all)]
    #![allow(clippy::all)]
    #![allow(clippy::pedantic)]
    include!(concat!(env!("OUT_DIR"), "/perfetto.protos.rs"));
}
