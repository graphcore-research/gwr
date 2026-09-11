// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_config::multi_source_config;

#[multi_source_config]
struct Config {
    #[arg(long, default_value_if("mode", "special", "output"))]
    output: Option<String>,
}

fn main() {}
