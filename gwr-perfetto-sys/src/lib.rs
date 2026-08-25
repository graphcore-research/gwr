// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

#![doc(test(attr(deny(unused_must_use))))]
#![doc = std::include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]

mod submodule_path;

pub use submodule_path::PERFETTO_SOURCE;
