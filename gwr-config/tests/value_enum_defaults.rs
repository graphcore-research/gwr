// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use clap::{CommandFactory, Parser};
use gwr_config::multi_source_config;
use serde::{Deserialize, Serialize};

// No Debug or Display: ValueEnum defaults must not acquire either bound.
#[derive(Clone, Default, Serialize, Deserialize, clap::ValueEnum)]
enum Mode {
    PublicMode,
    #[default]
    #[value(skip)]
    Internal,
}

#[multi_source_config]
struct Config {
    #[arg(long, value_enum, default_value_t = Mode::Internal)]
    mode: Mode,
    #[arg(long, value_enum, default_value_t = Mode::Internal)]
    optional: Option<Mode>,
    #[arg(long, value_enum, default_value_t = Mode::PublicMode)]
    visible: Mode,
}

#[test]
fn command_construction_allows_skipped_typed_fallbacks() {
    ConfigPartial::command_for_update().debug_assert();
    Config::command().debug_assert();
    let partial = ConfigPartial::try_parse_from(["test"]).unwrap();
    assert!(partial.mode.is_none());
    let config = Config::partial_to_config(partial);
    assert!(matches!(config.mode, Mode::Internal));
    assert!(matches!(config.optional, Some(Mode::Internal)));
}

#[test]
fn help_describes_defaults_without_a_cli_spelling() {
    let error = ConfigPartial::try_parse_from(["test", "--help"])
        .err()
        .unwrap();
    assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
    let help = error.to_string();
    assert!(help.contains("[default: non-CLI value]"), "{help}");
    assert!(help.contains("[default: public-mode]"), "{help}");
}

#[test]
fn cli_values_override_skipped_typed_fallbacks() {
    let partial = ConfigPartial::try_parse_from([
        "test",
        "--mode",
        "public-mode",
        "--optional",
        "public-mode",
    ])
    .unwrap();
    let config = Config::partial_to_config(partial);
    assert!(matches!(config.mode, Mode::PublicMode));
    assert!(matches!(config.optional, Some(Mode::PublicMode)));
    let error = ConfigPartial::try_parse_from(["test", "--mode", "internal"])
        .err()
        .unwrap();
    assert_eq!(error.kind(), clap::error::ErrorKind::InvalidValue);
}
