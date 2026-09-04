// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#![doc(test(attr(deny(unused_must_use))))]
#![doc = std::include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]

/// Auto-generated ONNX module
///
/// The contents of this file are created by `build.rs` at compile-time. They
/// provide all the functions required to access [ONNX](https://onnx.ai/)
/// types defined as part of the
/// [ONNX Intermediate Representation](https://github.com/onnx/onnx/blob/main/onnx/onnx.in.proto).
pub mod protos {
    // No need to emit warnings for auto-generated Protobuf code
    #![allow(missing_docs)]
    #![allow(rustdoc::all)]
    #![allow(clippy::all)]
    #![allow(clippy::pedantic)]
    include!(concat!(env!("OUT_DIR"), "/onnx.rs"));
}
