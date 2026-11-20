// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use clap::Parser;
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
fn direct_parsing_applies_skipped_enum_fallbacks() {
    let config = Config::try_parse_from(["test"]).unwrap();
    assert!(matches!(config.mode, Mode::Internal));
    assert!(matches!(config.optional, Some(Mode::Internal)));
    assert!(matches!(config.visible, Mode::PublicMode));
}
