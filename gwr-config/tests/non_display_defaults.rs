// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use clap::Parser;
use gwr_config::multi_source_config;
use serde::{Deserialize, Serialize};

// Deliberately no Display: typed fallbacks must never be converted to CLI text.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Count(u64);

#[multi_source_config]
struct Config {
    #[arg(long, default_value_t = Count(7), value_parser = parse_count)]
    count: Count,
    #[arg(long, default_value_t = Count(8), value_parser = parse_count)]
    optional: Option<Count>,
}

#[derive(Parser)]
struct Outer {
    #[command(flatten)]
    config: Config,
}

fn parse_count(input: &str) -> Result<Count, String> {
    input
        .strip_prefix("count:")
        .ok_or_else(|| "expected count:N".to_owned())?
        .parse::<u64>()
        .map(Count)
        .map_err(|error| error.to_string())
}

#[test]
fn typed_defaults_need_no_display_or_cli_spelling() {
    let parsed = Config::default();
    assert_eq!(parsed.count, Count(7));
    assert_eq!(parsed.optional, Some(Count(8)));
}

#[test]
fn flattened_custom_values_override_typed_defaults() {
    let parsed = Outer::try_parse_from(["test", "--optional", "count:12"]).unwrap();
    assert_eq!(parsed.config.count, Count(7));
    assert_eq!(parsed.config.optional, Some(Count(12)));
}

#[cfg(unix)]
#[test]
fn path_defaults_preserve_non_utf8_bytes() {
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    #[multi_source_config]
    struct NonUtf8 {
        #[arg(long, default_value_t = PathBuf::from(std::ffi::OsString::from_vec(vec![b'p', 0xff])))]
        path: Option<PathBuf>,
    }

    let parsed = NonUtf8::try_parse_from(["test"]).unwrap();
    assert_eq!(
        parsed.path.unwrap().into_os_string().into_vec(),
        vec![b'p', 0xff]
    );
}
