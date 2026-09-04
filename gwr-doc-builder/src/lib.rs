// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

#![doc(test(attr(deny(unused_must_use))))]
#![doc = std::include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]

pub mod asciidoctor_pp;
pub mod doc_nodes;
pub mod doc_parser;
pub mod helpers;
pub mod toc;
