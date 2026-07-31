// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

const INVALID: &str = gwr_docpp::include_str!("gwr-docpp/tests/fixtures/does-not-exist.txt");

fn main() {
    let _ = INVALID;
}
