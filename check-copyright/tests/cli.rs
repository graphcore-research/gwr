// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;
use std::io::Write;
use std::process::Command;

use tempfile::{Builder, NamedTempFile, tempdir};

const SUCCESS_STATUS: i32 = 0;
const FAILURE_STATUS: i32 = 1;
const LICENSE_FILENAME: &str = "LICENSE";

const GENERAL_CORRECT: &str = "// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::path::PathBuf;";

const GENERAL_INCORRECT_1: &str = "// Copyright (c) 2025 Graphcore Ltd. All rights xreserved.

use std::path::PathBuf;";

const GENERAL_INCORRECT_2: &str = "use std::path::PathBuf;
use std::process::exit;
use std::{fs, io};";

const LICENSE_CORRECT: &str = "MIT License

Copyright (c) 2025 Graphcore Ltd.

Permission is hereby granted...";

const LICENSE_INCORRECT_1: &str = "MIT License

Copyright (c) 2025 Graphcore Ltd. All rights reserved.

Permission is hereby granted...";

const LICENSE_INCORRECT_2: &str = "MIT License

Permission is hereby granted...";

#[test]
fn no_arg() {
    let run = Command::new(env!("CARGO_BIN_EXE_check-copyright"))
        .output()
        .expect("failed to execute process");

    assert_ne!(
        run.status.code().expect("exit code not signal"),
        SUCCESS_STATUS
    );
}

#[test]
fn invalid_filename() {
    let run = Command::new(env!("CARGO_BIN_EXE_check-copyright"))
        .arg("")
        .output()
        .expect("failed to execute process");

    assert_eq!(
        run.status.code().expect("exit code not signal"),
        FAILURE_STATUS
    );
}

#[test]
fn directory() {
    let tmp_dir = tempdir().expect("test should be able to create a tempdir");
    let run = Command::new(env!("CARGO_BIN_EXE_check-copyright"))
        .arg(tmp_dir.path())
        .output()
        .expect("failed to execute process");

    assert_eq!(
        run.status.code().expect("exit code not signal"),
        FAILURE_STATUS
    );
}

#[test]
fn single_correct_file() {
    let mut file = NamedTempFile::new().expect("test should be able to create a tempfile");
    writeln!(file, "{GENERAL_CORRECT}").expect("test should be able to write to tempfile");

    let run = Command::new(env!("CARGO_BIN_EXE_check-copyright"))
        .arg(file.path())
        .output()
        .expect("failed to execute process");

    assert_eq!(
        run.status.code().expect("exit code not signal"),
        SUCCESS_STATUS
    );
}

#[test]
fn single_correct_license() {
    let tmp_dir = tempdir().expect("test should be able to create a tempdir");
    let mut file = Builder::new()
        .prefix(LICENSE_FILENAME)
        .rand_bytes(0)
        .suffix("")
        .tempfile_in(&tmp_dir)
        .expect("test should be able to create to tempfile");
    writeln!(file, "{LICENSE_CORRECT}").expect("test should be able to write to tempfile");

    let run = Command::new(env!("CARGO_BIN_EXE_check-copyright"))
        .arg(file.path())
        .output()
        .expect("failed to execute process");

    assert_eq!(
        run.status.code().expect("exit code not signal"),
        SUCCESS_STATUS
    );
}

#[test]
fn multiple_correct_files() {
    let mut file_a = NamedTempFile::new().expect("test should be able to create a tempfile");
    writeln!(file_a, "{GENERAL_CORRECT}").expect("test should be able to write to tempfile");

    let mut file_b = NamedTempFile::new().expect("test should be able to create a tempfile");
    writeln!(file_b, "{GENERAL_CORRECT}").expect("test should be able to write to tempfile");

    let run = Command::new(env!("CARGO_BIN_EXE_check-copyright"))
        .args([file_a.path(), file_b.path()])
        .output()
        .expect("failed to execute process");

    assert_eq!(
        run.status.code().expect("exit code not signal"),
        SUCCESS_STATUS
    );
}

#[test]
fn single_incorrect_file_1() {
    let mut file = NamedTempFile::new().expect("test should be able to create a tempfile");
    writeln!(file, "{GENERAL_INCORRECT_1}").expect("test should be able to write to tempfile");

    let run = Command::new(env!("CARGO_BIN_EXE_check-copyright"))
        .arg(file.path())
        .output()
        .expect("failed to execute process");

    assert_eq!(
        run.status.code().expect("exit code not signal"),
        FAILURE_STATUS
    );
}

#[test]
fn single_incorrect_file_2() {
    let mut file = NamedTempFile::new().expect("test should be able to create a tempfile");
    writeln!(file, "{GENERAL_INCORRECT_2}").expect("test should be able to write to tempfile");

    let run = Command::new(env!("CARGO_BIN_EXE_check-copyright"))
        .arg(file.path())
        .output()
        .expect("failed to execute process");

    assert_eq!(
        run.status.code().expect("exit code not signal"),
        FAILURE_STATUS
    );
}

#[test]
fn single_incorrect_license_1() {
    let tmp_dir = tempdir().expect("test should be able to create a tempdir");
    let mut file = Builder::new()
        .prefix(LICENSE_FILENAME)
        .rand_bytes(0)
        .suffix("")
        .tempfile_in(&tmp_dir)
        .expect("test should be able to create to tempfile");
    writeln!(file, "{LICENSE_INCORRECT_1}").expect("test should be able to write to tempfile");

    let run = Command::new(env!("CARGO_BIN_EXE_check-copyright"))
        .arg(file.path())
        .output()
        .expect("failed to execute process");

    assert_eq!(
        run.status.code().expect("exit code not signal"),
        FAILURE_STATUS
    );
}

#[test]
fn single_incorrect_license_2() {
    let tmp_dir = tempdir().expect("test should be able to create a tempdir");
    let mut file = Builder::new()
        .prefix(LICENSE_FILENAME)
        .rand_bytes(0)
        .suffix("")
        .tempfile_in(&tmp_dir)
        .expect("test should be able to create to tempfile");
    writeln!(file, "{LICENSE_INCORRECT_2}").expect("test should be able to write to tempfile");

    let run = Command::new(env!("CARGO_BIN_EXE_check-copyright"))
        .arg(file.path())
        .output()
        .expect("failed to execute process");

    assert_eq!(
        run.status.code().expect("exit code not signal"),
        FAILURE_STATUS
    );
}

#[test]
fn correct_and_incorrect_files() {
    let mut file_a = NamedTempFile::new().expect("test should be able to create a tempfile");
    writeln!(file_a, "{GENERAL_CORRECT}").expect("test should be able to write to tempfile");

    let mut file_b = NamedTempFile::new().expect("test should be able to create a tempfile");
    writeln!(file_b, "{GENERAL_INCORRECT_1}").expect("test should be able to write to tempfile");

    let run = Command::new(env!("CARGO_BIN_EXE_check-copyright"))
        .args([file_a.path(), file_b.path()])
        .output()
        .expect("failed to execute process");

    assert_eq!(
        run.status.code().expect("exit code not signal"),
        FAILURE_STATUS
    );
}

#[test]
fn incorrect_and_correct_files() {
    let mut file_a = NamedTempFile::new().expect("test should be able to create a tempfile");
    writeln!(file_a, "{GENERAL_INCORRECT_2}").expect("test should be able to write to tempfile");

    let mut file_b = NamedTempFile::new().expect("test should be able to create a tempfile");
    writeln!(file_b, "{GENERAL_CORRECT}").expect("test should be able to write to tempfile");

    let run = Command::new(env!("CARGO_BIN_EXE_check-copyright"))
        .args([file_a.path(), file_b.path()])
        .output()
        .expect("failed to execute process");

    assert_eq!(
        run.status.code().expect("exit code not signal"),
        FAILURE_STATUS
    );
}

#[test]
fn relative_file_from_another_directory() {
    let tmp_dir = tempdir().expect("test should be able to create a tempdir");
    let file = tmp_dir.path().join("example.rs");
    fs::write(&file, GENERAL_CORRECT).expect("test should be able to write to tempfile");

    let run = Command::new(env!("CARGO_BIN_EXE_check-copyright"))
        .current_dir(tmp_dir.path())
        .arg("example.rs")
        .output()
        .expect("failed to execute process");

    assert_eq!(
        run.status.code().expect("exit code not signal"),
        SUCCESS_STATUS
    );
}
