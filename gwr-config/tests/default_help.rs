// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use clap::error::ErrorKind;
use clap::{Command, CommandFactory};
use gwr_config::multi_source_config;

#[multi_source_config(conf_file_flag = "settings")]
struct Config {
    /// Number of events.
    ///
    /// Processing stops after this many events
    /// have completed.
    #[arg(long, default_value_t = paragraphs())]
    events: u64,
}

#[multi_source_config]
struct ShortConfig {
    /// Number of workers.
    #[arg(long, default_value_t = 4)]
    workers: u64,
}

fn paragraphs() -> u64 {
    7
}

fn help(mut command: Command, flag: &str) -> String {
    let error = command
        .try_get_matches_from_mut(["test", flag])
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::DisplayHelp);
    error.to_string()
}

#[test]
fn layered_long_help_includes_typed_defaults() {
    let help = help(ConfigPartial::command(), "--help");
    assert!(help.contains("[default: 7]"), "{help}");
}

#[test]
fn long_help_preserves_doc_paragraphs() {
    let command = ConfigPartial::command();
    let argument = command
        .get_arguments()
        .find(|argument| argument.get_id() == "events")
        .unwrap();
    let help = argument.get_long_help().unwrap().to_string();
    assert!(
        help.starts_with(
            "Number of events.\n\nProcessing stops after this many events have completed."
        ),
        "{help}"
    );
}

#[test]
fn short_help_still_includes_typed_defaults() {
    let help = help(ConfigPartial::command(), "-h");
    assert!(help.contains("[default: 7]"), "{help}");
}

#[test]
fn single_paragraph_help_retains_the_short_help_fallback() {
    let command = ShortConfig::command();
    let argument = command
        .get_arguments()
        .find(|argument| argument.get_id() == "workers")
        .unwrap();
    assert!(argument.get_long_help().is_none());
    let help = help(command, "--help");
    assert!(help.contains("[default: 4]"), "{help}");
}
