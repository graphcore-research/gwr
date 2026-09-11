// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use clap::{CommandFactory, Parser};
use gwr_config::multi_source_config;

static NEXT_FILE: AtomicUsize = AtomicUsize::new(0);

struct File(PathBuf);
impl File {
    fn new(contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "gwr-subset-{}-{}.toml",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, contents).unwrap();
        Self(path)
    }
}
impl Drop for File {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).unwrap();
    }
}

fn run(path: &OsStr, args: &[&str], env: &[(&str, &str)], required: bool) -> Output {
    let mut command = Command::new("cargo");
    command
        .args([
            "run",
            "--quiet",
            "-p",
            "gwr-config",
            "--example",
            "application_subset",
            "--",
            "--conf-file",
        ])
        .arg(path)
        .args(args);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GWR_") {
            command.env_remove(key);
        }
    }
    command
        .env_remove("SUBSET_REQUIRED")
        .envs(env.iter().copied());
    if required {
        command.env("SUBSET_REQUIRED", "1");
    }
    command.output().unwrap()
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn precedence_and_sparse_booleans() {
    let file = File::new("count = 11\nverbose = true\nenabled = true\n");
    let text = success(run(
        file.0.as_os_str(),
        &["-C", "33", "--verbose=false"],
        &[("GWR_COUNT", "22")],
        false,
    ));
    assert!(text.contains("count: Some(33)"));
    assert!(text.contains("verbose: Some(false)"));
    assert!(text.contains("enabled: true"));
    assert!(text.contains("parser_calls=1"));
    let text = success(run(OsStr::new(""), &[], &[], false));
    assert!(text.contains("count: Some(7)"));
    assert!(text.contains("verbose: None"));
    assert!(text.contains("enabled: false"));
    assert!(text.contains("parser_calls=0"));
    assert!(
        success(run(OsStr::new(""), &["--verbose", "--enabled"], &[], false))
            .contains("verbose: Some(true)")
    );
}

#[test]
fn typed_deserializers_and_enum_sources() {
    for value in ["2048", "\"2KiB\""] {
        let file = File::new(&format!("bytes = {value}\nmode = \"random\"\n"));
        let text = success(run(file.0.as_os_str(), &[], &[], false));
        assert!(text.contains("bytes: Some(2048)"));
        assert!(text.contains("mode: Some(Random)"));
    }
    let text = success(run(OsStr::new(""), &[], &[("GWR_BYTES", "3KiB")], false));
    assert!(text.contains("bytes: Some(3072)"));
    let text = success(run(
        OsStr::new(""),
        &["--bytes", "4KiB", "--mode", "random"],
        &[],
        false,
    ));
    assert!(text.contains("bytes: Some(4096)"));
    assert!(text.contains("mode: Some(Random)"));
}

#[test]
fn overridden_invalid_values_are_not_deserialized() {
    let file = File::new("count = \"invalid\"\nbytes = \"invalid\"\n");
    let text = success(run(
        file.0.as_os_str(),
        &["--bytes", "5KiB"],
        &[("GWR_COUNT", "42"), ("GWR_BYTES", "invalid")],
        false,
    ));
    assert!(text.contains("count: Some(42)"));
    assert!(text.contains("bytes: Some(5120)"));
    assert!(!run(file.0.as_os_str(), &[], &[], false).status.success());
}

#[test]
fn exclusive_flat_group_replaces_lower_selection_only() {
    let file = File::new("log = false\ncount = 11\n");
    let text = success(run(
        file.0.as_os_str(),
        &[],
        &[("GWR_BIN", "input.bin")],
        false,
    ));
    assert!(text.contains("log: None, bin: Some(\"input.bin\")"));
    assert!(text.contains("count: Some(11)"));
    let text = success(run(
        file.0.as_os_str(),
        &["--log", "input.log"],
        &[("GWR_BIN", "input.bin")],
        false,
    ));
    assert!(text.contains("log: Some(\"input.log\"), bin: None"));
    let file = File::new("log = \"a\"\nbin = \"b\"\n");
    let output = run(file.0.as_os_str(), &[], &[], false);
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("--log") && error.contains("--bin"),
        "{error}"
    );
    assert!(
        !run(OsStr::new(""), &["--log", "a", "--bin", "b"], &[], false)
            .status
            .success()
    );
}

#[test]
fn required_vector_is_checked_after_merging() {
    assert!(!run(OsStr::new(""), &[], &[], true).status.success());
    let file = File::new("files = []\n");
    assert!(!run(file.0.as_os_str(), &[], &[], true).status.success());
    assert!(
        success(run(file.0.as_os_str(), &["a.rs", "b.rs"], &[], true))
            .contains("[\"a.rs\", \"b.rs\"]")
    );
    assert!(success(run(OsStr::new(""), &[], &[("GWR_FILES", "[a.rs]")], true)).contains("a.rs"));
}

#[test]
fn explicit_file_errors_and_help_without_loading_sources() {
    let missing = std::env::temp_dir().join(format!("gwr-missing-{}.toml", std::process::id()));
    assert!(!run(missing.as_os_str(), &[], &[], false).status.success());
    assert!(
        !run(std::env::temp_dir().as_os_str(), &[], &[], false)
            .status
            .success()
    );
    let help = success(run(
        missing.as_os_str(),
        &["--help"],
        &[("GWR_COUNT", "invalid")],
        false,
    ));
    assert!(help.contains("Number of events. [default: 7]"));
    assert!(help.contains("[default: round-robin]"));
    assert!(help.contains("--conf-file"));
    let malformed = File::new("[not valid toml");
    assert!(
        !run(malformed.0.as_os_str(), &[], &[], false)
            .status
            .success()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn native_file_paths_and_cfg_fields() {
    use std::os::unix::ffi::OsStringExt;
    let mut file = File::new("count = 19");
    let path = file.0.with_file_name(std::ffi::OsString::from_vec(
        format!("gwr-native-{}-", std::process::id())
            .into_bytes()
            .into_iter()
            .chain([0xff])
            .collect(),
    ));
    std::fs::rename(&file.0, &path).unwrap();
    file.0 = path;
    let text = success(run(
        file.0.as_os_str(),
        &["--unix-option", "present"],
        &[],
        false,
    ));
    assert!(text.contains("count: Some(19)"));
    assert!(text.contains("unix_option: Some(\"present\")"));
}

#[cfg(unix)]
#[test]
fn non_utf8_flag_is_parsed_without_string_conversion() {
    use std::os::unix::ffi::OsStringExt;
    // macOS filesystems reject invalid UTF-8 filenames, so test parsing without
    // trying to create one. Linux additionally exercises loading such a file.
    let path = std::ffi::OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0xff]);
    assert!(run(&path, &["--help"], &[], false).status.success());
    let output = run(&path, &[], &[], false);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not found"));
    let text = success(run(
        OsStr::new(""),
        &["--unix-option", "present"],
        &[],
        false,
    ));
    assert!(text.contains("unix_option: Some(\"present\")"));
}

#[multi_source_config]
#[derive(Debug, PartialEq)]
struct Leaf {
    #[arg(long, default_value_t = 9)]
    value: Option<u64>,
}

#[test]
fn partials_are_sparse_but_leaf_defaults_are_typed() {
    assert_eq!(LeafPartial::try_parse_from(["test"]).unwrap().value, None);
    assert_eq!(Leaf::default().value, Some(9));
    LeafPartial::command().debug_assert();
    <Leaf as clap::Args>::augment_args(clap::Command::new("test")).debug_assert();
}
