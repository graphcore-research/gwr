// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! The supported contract, mirroring options used by the workspace
//! applications.

#![allow(dead_code)] // The fixture deliberately prints entire configurations.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use gwr_config::multi_source_config;
use serde::{Deserialize, Deserializer, Serialize};

static PARSES: AtomicUsize = AtomicUsize::new(0);

fn parse_count(text: &str) -> Result<u64, std::num::ParseIntError> {
    PARSES.fetch_add(1, Ordering::Relaxed);
    text.parse()
}

fn parse_bytes(text: &str) -> Result<usize, String> {
    let (digits, scale) = text.strip_suffix("KiB").map_or((text, 1), |n| (n, 1024));
    digits
        .parse::<usize>()
        .map_err(|e| e.to_string())?
        .checked_mul(scale)
        .ok_or("byte count overflow".into())
}

fn deserialize_bytes<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<usize>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Bytes {
        Number(usize),
        Text(String),
    }
    Option::<Bytes>::deserialize(deserializer)?
        .map(|b| match b {
            Bytes::Number(n) => Ok(n),
            Bytes::Text(text) => parse_bytes(&text).map_err(serde::de::Error::custom),
        })
        .transpose()
}

#[derive(Clone, Debug, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum Mode {
    RoundRobin,
    Random,
}

#[multi_source_config]
#[derive(Debug)]
#[group(id = "input", multiple = false)]
struct Input {
    #[arg(long)]
    log: Option<PathBuf>,
    #[arg(long)]
    bin: Option<PathBuf>,
}

#[multi_source_config(
    default_conf_file = "absent-default.toml",
    conf_file_flag = "conf-file"
)]
#[derive(Debug)]
#[command(about = "Application-focused configuration")]
struct Config {
    #[command(flatten)]
    #[serde(flatten)]
    input: Input,
    /// Number of events.
    #[arg(short = 'C', long, default_value_t = 7, value_parser = parse_count)]
    count: Option<u64>,
    #[arg(long, default_value_t = 1024, value_parser = parse_bytes)]
    #[serde(default, deserialize_with = "deserialize_bytes")]
    bytes: Option<usize>,
    #[arg(long, value_enum, default_value_t = Mode::RoundRobin)]
    mode: Option<Mode>,
    #[arg(long)]
    verbose: Option<bool>,
    #[arg(long, default_value_t = false)]
    enabled: bool,
    #[cfg(unix)]
    #[arg(long)]
    unix_option: Option<String>,
}

#[multi_source_config(conf_file_flag = "conf-file")]
#[derive(Debug)]
struct Required {
    #[clap(required = true)]
    files: Vec<String>,
}

fn main() {
    if std::env::var_os("SUBSET_REQUIRED").is_some() {
        println!("{:?}", Required::parse_all_sources());
    } else {
        println!("{:?}", Config::parse_all_sources());
        println!("parser_calls={}", PARSES.load(Ordering::Relaxed));
    }
}
