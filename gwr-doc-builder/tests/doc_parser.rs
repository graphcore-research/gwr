// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::io::Write;

use gwr_doc_builder::doc_parser::DocParser;
use tempfile::NamedTempFile;

#[test]
fn accepts_a_verbatim_macro_item() {
    let mut input = NamedTempFile::new().expect("failed to create Rust source fixture");
    input
        .write_all(b"macro name() {}")
        .expect("failed to write Rust source fixture");

    let mut parser = DocParser::new(true);
    let root = parser.parse_doc(
        input
            .path()
            .to_str()
            .expect("temporary path must be valid UTF-8"),
    );

    assert_eq!(root.borrow().full_name(), "crate");
}
