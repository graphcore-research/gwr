// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use figment::Figment;
use figment::providers::{Format, Toml};
use gwr_config::multi_source_config;
use serde::{Deserialize, Deserializer};

#[multi_source_config(conf_file_flag = "settings")]
struct Config {
    #[arg(long, default_value_t = 7)]
    #[serde(deserialize_with = "number")]
    count: u64,
    #[cfg(any())]
    #[arg(long, default_value_t = 0)]
    #[serde(deserialize_with = "missing_function")]
    absent: MissingType,
}

fn number<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    String::deserialize(deserializer)?
        .parse()
        .map_err(serde::de::Error::custom)
}

#[test]
fn absent_scalar_values_remain_sparse_until_defaults_are_applied() {
    let partial: ConfigPartial = Figment::from(Toml::string("")).extract().unwrap();
    assert_eq!(partial.count, None);
    assert_eq!(Config::partial_to_config(partial).count, 7);
}

#[test]
fn scalar_deserializers_are_adapted_to_partial_values() {
    let partial: ConfigPartial = Figment::from(Toml::string("count = '17'"))
        .extract()
        .unwrap();
    assert_eq!(partial.count, Some(17));
    assert_eq!(Config::partial_to_config(partial).count, 17);
}

#[test]
fn final_configs_retain_scalar_deserializers() {
    let config: Config = Figment::from(Toml::string("count = '17'"))
        .extract()
        .unwrap();
    assert_eq!(config.count, 17);
}

#[test]
fn scalar_deserializers_reject_invalid_source_values() {
    assert!(
        Figment::from(Toml::string("count = 'invalid'"))
            .extract::<ConfigPartial>()
            .is_err()
    );
}
