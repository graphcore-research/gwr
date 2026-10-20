// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_config::multi_source_config;

#[multi_source_config(conf_file_flag = "settings")]
struct Explicit {
    #[arg(long = "--settings")]
    file: Option<String>,
}

#[multi_source_config(conf_file_flag = "--settings")]
struct SingleHyphen {
    #[clap(long = "-settings")]
    file: Option<String>,
}

#[multi_source_config]
struct Child {
    #[arg(long = "---settings")]
    file: Option<String>,
}

#[multi_source_config(conf_file_flag = "settings")]
struct Flattened {
    #[command(flatten)]
    #[serde(flatten)]
    child: Child,
}

fn main() {}
