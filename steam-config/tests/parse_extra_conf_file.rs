// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::{env, path::PathBuf};

use steam_config::multi_source_config;

#[multi_source_config]
#[derive(Debug, PartialEq)]
struct Config {
    /// First
    #[arg(long)]
    a: Option<String>,

    /// Second
    #[arg(long)]
    b: Option<bool>,

    /// Third
    ///
    /// This happens to have a long help entry too.
    #[arg(long)]
    c: Option<u64>,

    #[arg(long)]
    d: Option<String>,

    /// Path to additional configuration file
    ///
    /// This additional configuration file must contain TOML, and set values for
    /// fields of this struct.
    #[arg(long)]
    conf_file: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            a: Some(Default::default()),
            b: Some(Default::default()),
            c: Some(Default::default()),
            d: Some("foo".to_string()),
            conf_file: Some(Default::default()),
        }
    }
}

#[test]
fn default_value() {
    let mut config = Config::parse_all_sources();
    let extra_conf_file = config.conf_file.clone().unwrap();
    let result = config.parse_extra_conf_file(&extra_conf_file);

    assert!(result.is_ok());
}

#[test]
fn multiple_extra_files() {
    let mut config = Config::parse_all_sources();
    let mut test_dir = env::current_dir().unwrap();
    test_dir.push("tests");
    let extra_conf_files = [
        test_dir.join("partial_conf_file1.toml"),
        test_dir.join("partial_conf_file2.toml"),
        test_dir.join("partial_conf_file3.toml"),
    ];
    for extra_conf_file in extra_conf_files {
        let result = config.parse_extra_conf_file(&extra_conf_file);
        assert!(result.is_ok());
    }

    let expected = Config {
        a: Some("bar".to_string()),
        b: Some(true),
        c: Some(300),
        d: Some("boo".to_string()),
        conf_file: Some(Default::default()),
    };
    assert_eq!(config, expected);
}

#[test]
fn file_not_found() {
    let mut config = Config::parse_all_sources();
    let extra_conf_file = PathBuf::from("foo.toml");
    let result = config.parse_extra_conf_file(&extra_conf_file);

    let result_error = result.map_err(|e| e.kind());
    let expected = Err(std::io::ErrorKind::NotFound);
    assert_eq!(result_error, expected);
}

#[test]
fn path_is_directory() {
    let mut config = Config::parse_all_sources();
    let extra_conf_file = PathBuf::from(".");
    let result = config.parse_extra_conf_file(&extra_conf_file);

    let result_error = result.map_err(|e| e.kind());
    let expected = Err(std::io::ErrorKind::IsADirectory);
    assert_eq!(result_error, expected);
}
