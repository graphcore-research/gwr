// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use clap::{CommandFactory, FromArgMatches, Parser};
use gwr_config::multi_source_config;

#[multi_source_config]
struct RequiredLeaf {
    #[arg(long, default_value_t = 9)]
    value: u64,
}

#[multi_source_config]
struct OptionalLeaf {
    #[clap(long, default_value_t = 9)]
    value: Option<u64>,
    #[arg(long, default_value_t = true)]
    enabled: Option<bool>,
    #[cfg(any())]
    absent: Option<MissingType>,
}

#[derive(Parser)]
struct MaybeOuter {
    #[command(flatten)]
    leaf: Option<OptionalLeaf>,
}

#[multi_source_config]
struct ValidatedLeaf {
    #[arg(long, default_value_t = 7, value_parser = clap::value_parser!(u64).range(10..))]
    value: u64,
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u64).range(10..))]
    optional: Option<u64>,
}

#[multi_source_config]
#[group(id = "input", multiple = false)]
struct Input {
    #[arg(long)]
    log: Option<String>,
    #[arg(long)]
    bin: Option<String>,
}

#[derive(Parser)]
struct InputOuter {
    #[command(flatten)]
    input: Option<Input>,
}

#[multi_source_config(conf_file_flag = "settings")]
struct WithFileFlag {
    #[arg(long, default_value_t = 9)]
    value: u64,
}

#[multi_source_config]
#[group(multiple = false)]
struct ImplicitInput {
    #[arg(long)]
    text: Option<String>,
}

#[derive(Parser)]
struct ImplicitGroupOuter {
    #[command(flatten)]
    input: ImplicitInput,
    #[arg(long, requires = "ImplicitInput")]
    require_input: bool,
}

#[test]
fn typed_defaults_bypass_cli_parsers() {
    let parsed = ValidatedLeaf::try_parse_from(["test"]).unwrap();
    assert_eq!(parsed.value, 7);
    assert_eq!(parsed.optional, Some(8));
}

#[test]
fn explicit_values_are_validated_by_cli_parsers() {
    assert!(ValidatedLeaf::try_parse_from(["test", "--value", "7"]).is_err());
    assert!(ValidatedLeaf::try_parse_from(["test", "--optional", "8"]).is_err());
}

#[test]
fn matches_stay_sparse_until_config_construction() {
    let mut matches = OptionalLeaf::command()
        .try_get_matches_from(["test"])
        .unwrap();
    assert!(!matches.contains_id("value"));
    assert_eq!(matches.value_source("enabled"), None);
    let config = OptionalLeaf::from_arg_matches_mut(&mut matches).unwrap();
    assert_eq!(config.value, Some(9));
    assert_eq!(config.enabled, Some(true));
}

#[test]
fn mutable_parsing_consumes_explicit_matches() {
    let mut matches = OptionalLeaf::command()
        .try_get_matches_from(["test", "--value", "12", "--enabled=false"])
        .unwrap();
    let config = OptionalLeaf::from_arg_matches_mut(&mut matches).unwrap();
    assert_eq!(config.value, Some(12));
    assert_eq!(config.enabled, Some(false));
    assert!(matches.get_one::<u64>("value").is_none());
    assert!(matches.get_one::<bool>("enabled").is_none());
}

#[test]
fn updates_preserve_omitted_values() {
    let mut config = OptionalLeaf {
        value: Some(21),
        enabled: Some(false),
    };
    config.try_update_from(["test"]).unwrap();
    assert_eq!(config.value, Some(21));
    assert_eq!(config.enabled, Some(false));
    config.try_update_from(["test", "--value", "22"]).unwrap();
    assert_eq!(config.value, Some(22));
    assert_eq!(config.enabled, Some(false));
}

#[test]
fn required_leaf_updates_preserve_omitted_values() {
    let mut config = RequiredLeaf { value: 31 };
    config.try_update_from(["test"]).unwrap();
    assert_eq!(config.value, 31);
    config.try_update_from(["test", "--value", "32"]).unwrap();
    assert_eq!(config.value, 32);
}

#[test]
fn mutable_updates_consume_values_and_readonly_updates_preserve_matches() {
    let mut config = OptionalLeaf {
        value: Some(21),
        enabled: Some(false),
    };
    let mut matches = OptionalLeaf::command_for_update()
        .try_get_matches_from(["test", "--value", "22"])
        .unwrap();
    config.update_from_arg_matches(&matches).unwrap();
    assert_eq!(matches.get_one::<u64>("value"), Some(&22));
    config.update_from_arg_matches_mut(&mut matches).unwrap();
    assert!(matches.get_one::<u64>("value").is_none());
    assert_eq!(config.value, Some(22));
    assert_eq!(config.enabled, Some(false));
}

#[test]
fn optional_flattening_applies_defaults_only_when_present() {
    assert!(MaybeOuter::try_parse_from(["test"]).unwrap().leaf.is_none());
    let mut outer = MaybeOuter::try_parse_from(["test", "--enabled=false"]).unwrap();
    let leaf = outer.leaf.as_ref().unwrap();
    assert_eq!(leaf.value, Some(9));
    assert_eq!(leaf.enabled, Some(false));
    outer.try_update_from(["test", "--value", "12"]).unwrap();
    let leaf = outer.leaf.as_ref().unwrap();
    assert_eq!(leaf.value, Some(12));
    assert_eq!(leaf.enabled, Some(false));
}

#[test]
fn exclusive_group_updates_replace_previous_selection() {
    let mut config = Input::try_parse_from(["test", "--log", "old.log"]).unwrap();
    config.try_update_from(["test"]).unwrap();
    assert_eq!(config.log.as_deref(), Some("old.log"));
    config
        .try_update_from(["test", "--bin", "new.bin"])
        .unwrap();
    assert!(config.log.is_none());
    assert_eq!(config.bin.as_deref(), Some("new.bin"));
    assert!(Input::try_parse_from(["test", "--log", "a", "--bin", "b"]).is_err());
}

#[test]
fn optional_flattening_preserves_group_identity_and_presence() {
    assert_eq!(<Input as clap::Args>::group_id().unwrap().as_str(), "input");
    assert!(
        InputOuter::try_parse_from(["test"])
            .unwrap()
            .input
            .is_none()
    );
    let parsed = InputOuter::try_parse_from(["test", "--log", "input.log"]).unwrap();
    assert_eq!(parsed.input.unwrap().log.as_deref(), Some("input.log"));
}

#[test]
fn direct_parser_does_not_expose_layered_file_flag() {
    assert!(WithFileFlag::try_parse_from(["test", "--settings", "missing.toml"]).is_err());
}

#[test]
fn parent_commands_can_reference_inferred_group_ids() {
    ImplicitGroupOuter::command().debug_assert();
    assert!(ImplicitGroupOuter::try_parse_from(["test", "--require-input"]).is_err());
    let parsed =
        ImplicitGroupOuter::try_parse_from(["test", "--require-input", "--text", "hello"]).unwrap();
    assert!(parsed.require_input);
    assert_eq!(parsed.input.text.as_deref(), Some("hello"));
}
