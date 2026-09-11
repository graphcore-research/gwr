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

#[multi_source_config(conf_file_flag = "version")]
struct VersionFile {}

#[multi_source_config(conf_file_flag = "settings")]
#[group(multiple = true)]
struct __GwrConfigCli {
    #[arg(long)]
    value: Option<u64>,
}

mod result_alias {
    use clap::Parser;
    use gwr_config::multi_source_config;

    type Result<T> = std::result::Result<T, clap::Error>;

    #[multi_source_config]
    struct Config {
        #[arg(long)]
        value: Option<u64>,
    }

    #[test]
    fn parsing_works_with_an_application_result_alias() {
        let config: Result<Config> = Config::try_parse_from(["test", "--value", "13"]);
        assert_eq!(config.unwrap().value, Some(13));
    }
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

#[test]
fn version_is_not_reserved_without_a_version_option() {
    VersionFilePartial::command().debug_assert();
    let parsed = VersionFilePartial::try_parse_from(["test", "--version", "custom.toml"]).unwrap();
    assert_eq!(
        parsed.__gwr_config_conf_file,
        std::path::PathBuf::from("custom.toml")
    );
    VersionFile::command().debug_assert();
    VersionFile::try_parse_from(["test"]).unwrap();
}

#[test]
fn direct_parser_helper_does_not_shadow_the_configuration_type() {
    assert_eq!(
        <__GwrConfigCli as clap::Args>::group_id().unwrap().as_str(),
        "__GwrConfigCli"
    );
    let config = __GwrConfigCli::try_parse_from(["test", "--value", "12"]).unwrap();
    assert_eq!(config.value, Some(12));
}
