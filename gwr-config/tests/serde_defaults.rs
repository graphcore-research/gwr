// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use clap::Parser;
use figment::Figment;
use figment::providers::{Format, Toml};
use gwr_config::multi_source_config;

#[multi_source_config]
struct Config {
    #[arg(long)]
    #[serde(default)]
    count: u64,
    #[arg(long, default_value_t = 7)]
    #[serde(default)]
    limit: u64,
    #[arg(long)]
    #[serde(default)]
    optional: Option<u64>,
}

fn source(toml: &str) -> ConfigPartial {
    Config::figment_extract(Figment::from(Toml::string(toml)))
}

#[test]
fn serde_default_is_applied_when_all_sources_omit_the_field() {
    let file = source("");
    assert_eq!(file.count, None);
    let cli = ConfigPartial::try_parse_from(["test"]).unwrap();
    assert_eq!(cli.count, None);
    let config = Config::partial_to_config(Config::clap_merge(file, cli));
    assert_eq!(config.count, 0);
    assert_eq!(config.limit, 7);
    assert_eq!(config.optional, None);
}

#[test]
fn omitted_higher_priority_values_do_not_replace_explicit_values_with_defaults() {
    let cli = ConfigPartial::try_parse_from(["test"]).unwrap();
    let config = Config::partial_to_config(Config::clap_merge(source("count = 23"), cli));
    assert_eq!(config.count, 23);

    let cli = ConfigPartial::try_parse_from(["test", "--count", "0"]).unwrap();
    let config = Config::partial_to_config(Config::clap_merge(source("count = 23"), cli));
    assert_eq!(config.count, 0);
}
