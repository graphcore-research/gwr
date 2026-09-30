// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_config::multi_source_config;

#[multi_source_config(conf_file_flag = "conf-file")]
struct Inferred {
    #[arg(long)]
    conf_file: Option<std::path::PathBuf>,
}

#[multi_source_config(conf_file_flag = "--settings")]
struct Explicit {
    #[clap(long = "settings")]
    file: Option<String>,
}

#[multi_source_config(conf_file_flag = "help")]
struct Reserved {}

#[multi_source_config(conf_file_flag = "type")]
struct RawIdentifier {
    #[arg(long)]
    r#type: Option<String>,
}

#[multi_source_config(conf_file_flag = "http-config")]
#[allow(non_snake_case)]
struct Acronym {
    #[arg(long)]
    HTTPConfig: Option<String>,
}

#[multi_source_config]
struct Child {
    #[arg(long = "settings")]
    path: Option<String>,
}

#[multi_source_config(conf_file_flag = "settings")]
struct Flattened {
    #[command(flatten)]
    #[serde(flatten)]
    child: Child,
}

#[multi_source_config]
struct NonLiteral {
    #[arg(long = concat!("set", "tings"))]
    value: Option<String>,
}

fn main() {}
