// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

const INVALID: &str = gwr_docpp::toc!(
    = "Guide"
    [
        - "Overview", 42
    ]
);

fn main() {
    let _ = INVALID;
}
