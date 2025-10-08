// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::path::PathBuf;

use tramway_config::multi_source_config;

#[multi_source_config]
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

    /// Path to additional configuration file
    ///
    /// This additional configuration file must contain TOML, and set values for
    /// fields of this struct.
    #[arg(long)]
    conf_file: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            a: Some(Default::default()),
            b: Some(Default::default()),
            c: Some(Default::default()),
            d: Some("foo".to_string()),
            conf_file: Some(Default::default()),
        }
    }
}

fn main() -> Result<(), std::io::Error> {
    let mut config = Config::parse_all_sources();
    let extra_conf_file = config.conf_file.clone().unwrap();
    config.parse_extra_conf_file(&extra_conf_file)?;
    println!("{config:#?}");

    Ok(())
}
