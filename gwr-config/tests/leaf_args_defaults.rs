// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use clap::{CommandFactory, Parser};
use gwr_config::multi_source_config;

#[multi_source_config]
struct RequiredLeaf {
    #[arg(long, default_value_t = 9)]
    value: u64,
}

#[derive(Parser)]
struct RequiredOuter {
    #[command(flatten)]
    leaf: RequiredLeaf,
}

#[multi_source_config]
struct OptionalLeaf {
    #[clap(long, default_value_t = 9)]
    value: Option<u64>,
    #[arg(long, default_value_t = true)]
    enabled: Option<bool>,
}

#[derive(Parser)]
struct OptionalOuter {
    #[command(flatten)]
    leaf: OptionalLeaf,
}

#[test]
fn flattened_required_leaf_uses_clap_default() {
    RequiredOuter::command().debug_assert();
    RequiredOuter::command_for_update().debug_assert();
    let parsed = RequiredOuter::try_parse_from(["test"]).unwrap();
    assert_eq!(parsed.leaf.value, 9);
    assert_eq!(RequiredLeaf::default().value, 9);
    let partial = RequiredLeafPartial::try_parse_from(["test"]).unwrap();
    assert!(partial.value.is_none());
    assert_eq!(RequiredLeaf::partial_to_config(partial).value, 9);
    let parsed = RequiredOuter::try_parse_from(["test", "--value", "12"]).unwrap();
    assert_eq!(parsed.leaf.value, 12);
}

#[test]
fn flattened_optional_leaf_uses_clap_defaults() {
    let parsed = OptionalOuter::try_parse_from(["test"]).unwrap();
    assert_eq!(parsed.leaf.value, Some(9));
    assert_eq!(parsed.leaf.enabled, Some(true));
    let partial = OptionalLeafPartial::try_parse_from(["test"]).unwrap();
    assert!(partial.value.is_none());
    assert!(partial.enabled.is_none());
    let parsed =
        OptionalOuter::try_parse_from(["test", "--value", "12", "--enabled=false"]).unwrap();
    assert_eq!(parsed.leaf.value, Some(12));
    assert_eq!(parsed.leaf.enabled, Some(false));
}
