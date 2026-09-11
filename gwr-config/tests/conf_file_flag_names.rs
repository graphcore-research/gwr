// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use clap::{CommandFactory, Parser};
use gwr_config::multi_source_config;

#[multi_source_config]
struct Child {
    #[arg(long)]
    child_value: Option<String>,
    #[cfg(any())]
    #[arg(long = "settings")]
    absent: Option<String>,
}

#[multi_source_config(conf_file_flag = "--settings")]
struct Config {
    #[arg(long = "input")]
    settings: Option<String>,
    #[cfg(any())]
    #[arg(long = "settings")]
    absent: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    child: Child,
}

#[test]
fn distinct_longs_and_disabled_fields_allow_the_file_flag() {
    ConfigPartial::command().debug_assert();
    ConfigPartial::command_for_update().debug_assert();
    let parsed = ConfigPartial::try_parse_from([
        "test",
        "--settings",
        "custom.toml",
        "--input",
        "value",
        "--child-value",
        "child",
    ])
    .unwrap();
    assert_eq!(
        parsed.__gwr_config_conf_file,
        std::path::PathBuf::from("custom.toml")
    );
    let config = Config::partial_to_config(parsed);
    assert_eq!(config.settings.as_deref(), Some("value"));
    assert_eq!(config.child.child_value.as_deref(), Some("child"));
}

#[multi_source_config(conf_file_flag = "version")]
struct VersionFile {}

#[test]
fn version_is_not_reserved_without_a_version_option() {
    VersionFilePartial::command().debug_assert();
    let parsed = VersionFilePartial::try_parse_from(["test", "--version", "custom.toml"]).unwrap();
    assert_eq!(
        parsed.__gwr_config_conf_file,
        std::path::PathBuf::from("custom.toml")
    );
}
