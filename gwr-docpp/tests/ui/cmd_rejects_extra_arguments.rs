// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

const INVALID: &str = gwr_docpp::cmd!("printf first", "printf second");

fn main() {
    let _ = INVALID;
}
