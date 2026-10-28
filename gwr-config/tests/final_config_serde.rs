// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use figment::Figment;
use figment::providers::{Format, Serialized, Toml};
use gwr_config::multi_source_config;
use serde::{Deserialize, Deserializer};

fn optional_number<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(|value| value.parse().map_err(serde::de::Error::custom))
        .transpose()
}

#[multi_source_config]
#[derive(Debug, PartialEq)]
struct RequiredConfig {
    #[arg(long)]
    name: String,
    #[arg(long)]
    #[serde(default, deserialize_with = "optional_number")]
    number: Option<u64>,
}

#[multi_source_config]
#[derive(Debug, PartialEq)]
struct Child {
    #[arg(long)]
    value: Option<u64>,
}

#[multi_source_config]
#[derive(Debug, PartialEq)]
struct FlattenedConfig {
    #[command(flatten)]
    #[serde(flatten)]
    child: Child,
}

#[test]
fn required_config_supports_serde_and_preserves_field_attributes() {
    let config: RequiredConfig = Figment::from(Toml::string("name = 'root'\nnumber = '42'"))
        .extract()
        .unwrap();
    assert_eq!(config.name, "root");
    assert_eq!(config.number, Some(42));
    let serialized = Figment::from(Serialized::defaults(&config));
    assert_eq!(serialized.extract_inner::<String>("name").unwrap(), "root");
    assert_eq!(serialized.extract_inner::<u64>("number").unwrap(), 42);

    let config: RequiredConfig = Figment::from(Toml::string("name = 'root'"))
        .extract()
        .unwrap();
    assert_eq!(config.number, None);
    assert!(
        Figment::from(Toml::string(""))
            .extract::<RequiredConfig>()
            .is_err()
    );
}

#[test]
fn flattened_config_supports_serde_with_flat_field_names() {
    let config: FlattenedConfig = Figment::from(Toml::string("value = 17")).extract().unwrap();
    assert_eq!(config.child.value, Some(17));
    let serialized = Figment::from(Serialized::defaults(&config));
    assert_eq!(serialized.extract_inner::<u64>("value").unwrap(), 17);
    assert_eq!(serialized.extract::<FlattenedConfig>().unwrap(), config);
}
