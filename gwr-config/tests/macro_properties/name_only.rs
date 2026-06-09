// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use gwr_config::multi_source_config;

#[multi_source_config(conf_file)]
#[derive(Debug)]
struct Config {
    /// First
    #[arg(long)]
    a: Option<String>,
}
