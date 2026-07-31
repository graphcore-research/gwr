// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

const INVALID: &str = gwr_docpp::toc!(
    = "Guide"
    [
        - 42, "crate"
    ]
);

fn main() {
    let _ = INVALID;
}
