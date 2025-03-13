// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use steam_config::multi_source_config;

#[multi_source_config(path = "foo.toml")]
#[derive(Debug)]
struct Config {
    /// First
    #[arg(long)]
    a: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            a: Default::default(),
        }
    }
}
