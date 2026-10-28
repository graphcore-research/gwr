// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::PathBuf;

use clap::Parser;
use gwr_config::multi_source_config;

#[multi_source_config]
struct Paths {
    #[arg(long, default_value_t = PathBuf::from("platform.yaml"))]
    platform: Option<PathBuf>,
    #[arg(long, default_value_t = PathBuf::from("timetable.yaml"))]
    timetable: PathBuf,
}

#[derive(Parser)]
struct Outer {
    #[command(flatten)]
    paths: Paths,
}

#[test]
fn flattened_paths_use_defaults_without_display() {
    let parsed = Outer::try_parse_from(["test"]).unwrap();
    assert_eq!(parsed.paths.platform, Some(PathBuf::from("platform.yaml")));
    assert_eq!(parsed.paths.timetable, PathBuf::from("timetable.yaml"));
    let partial = PathsPartial::try_parse_from(["test"]).unwrap();
    assert!(partial.platform.is_none());
    assert!(partial.timetable.is_none());
    let config = Paths::partial_to_config(partial);
    assert_eq!(config.platform, parsed.paths.platform);
    assert_eq!(config.timetable, parsed.paths.timetable);
    let parsed = Outer::try_parse_from(["test", "--platform", "custom.yaml"]).unwrap();
    assert_eq!(parsed.paths.platform, Some(PathBuf::from("custom.yaml")));
}

#[cfg(unix)]
#[test]
fn path_defaults_preserve_non_utf8_bytes() {
    use std::os::unix::ffi::OsStringExt;

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
