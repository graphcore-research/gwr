// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

const INVALID: &str = gwr_docpp::cmd!("gwr-docpp-command-that-does-not-exist");

fn main() {
    let _ = INVALID;
}
