// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_config::multi_source_config;

#[multi_source_config(conf_file_flag = "child-file")]
struct Child {
    #[arg(long)]
    count: Option<u64>,
}

#[multi_source_config]
struct Root {
    #[command(flatten)]
    #[serde(flatten)]
    child: Child,
}

fn main() {}
