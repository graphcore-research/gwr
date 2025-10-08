// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use tramway_config::multi_source_config;

#[multi_source_config(conf_file = "partial_conf_file.toml")]
#[derive(Debug)]
struct Config {
    /// First
    #[arg(long)]
    a: Option<String>,

    /// Second
    #[arg(long)]
    b: Option<bool>,

    /// Third
    ///
    /// This happens to have a long help entry too.
    #[arg(long)]
    c: Option<u64>,

    #[arg(long)]
    d: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            a: Some(Default::default()),
            b: Some(Default::default()),
            c: Some(Default::default()),
            d: Some("foo".to_string()),
        }
    }
}

fn main() {
    let config = Config::parse_all_sources();
    println!("{config:#?}");
}
