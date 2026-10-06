// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use tempfile::{TempDir, tempdir};

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

fn check_files(files: &[(&str, &str)]) -> Output {
    let directory = create_files(files);
    check_copyright()
        .args(
            files
                .iter()
                .map(|(filename, _)| directory.path().join(filename)),
        )
        .output()
        .expect("failed to execute process")
}

fn create_files(files: &[(&str, &str)]) -> TempDir {
    let directory = tempdir().expect("test should be able to create a tempdir");
    for (filename, contents) in files {
        fs::write(directory.path().join(filename), contents)
            .expect("test should be able to write a fixture");
    }
    directory
}

fn check_copyright() -> Command {
    Command::new(env!("CARGO_BIN_EXE_check-copyright"))
}

#[test]
fn no_arg() {
    let run = check_copyright()
        .output()
        .expect("failed to execute process");
    assert_ne!(
        run.status.code().expect("exit code not signal"),
        SUCCESS_STATUS
    );
}

#[test]
fn invalid_filename() {
    let run = check_copyright()
        .arg("")
        .output()
        .expect("failed to execute process");
    assert_eq!(run.status.code(), Some(FAILURE_STATUS));
}

#[test]
fn directory() {
    let directory = tempdir().expect("test should be able to create a tempdir");
    let run = check_copyright()
        .arg(directory.path())
        .output()
        .expect("failed to execute process");
    assert_eq!(run.status.code(), Some(FAILURE_STATUS));
}

#[test]
fn single_correct_file_accepts_absolute_and_relative_paths() {
    let directory = create_files(&[("example.rs", GENERAL_CORRECT)]);
    for path in [
        directory.path().join("example.rs"),
        PathBuf::from("example.rs"),
    ] {
        let run = check_copyright()
            .current_dir(directory.path())
            .arg(&path)
            .output()
            .expect("failed to execute process");
        assert_eq!(
            run.status.code(),
            Some(SUCCESS_STATUS),
            "failed for {}",
            path.display()
        );
    }
}

#[test]
fn single_correct_license() {
    let run = check_files(&[(LICENSE_FILENAME, LICENSE_CORRECT)]);
    assert_eq!(run.status.code(), Some(SUCCESS_STATUS));
}

#[test]
fn multiple_correct_files() {
    let run = check_files(&[
        ("first.rs", GENERAL_CORRECT),
        ("second.rs", GENERAL_CORRECT),
    ]);
    assert_eq!(run.status.code(), Some(SUCCESS_STATUS));
}

#[test]
fn single_incorrect_file_1() {
    let run = check_files(&[("example.rs", GENERAL_INCORRECT_1)]);
    assert_eq!(run.status.code(), Some(FAILURE_STATUS));
}

#[test]
fn single_incorrect_file_2() {
    let run = check_files(&[("example.rs", GENERAL_INCORRECT_2)]);
    assert_eq!(run.status.code(), Some(FAILURE_STATUS));
}

#[test]
fn single_incorrect_license_1() {
    let run = check_files(&[(LICENSE_FILENAME, LICENSE_INCORRECT_1)]);
    assert_eq!(run.status.code(), Some(FAILURE_STATUS));
}

#[test]
fn single_incorrect_license_2() {
    let run = check_files(&[(LICENSE_FILENAME, LICENSE_INCORRECT_2)]);
    assert_eq!(run.status.code(), Some(FAILURE_STATUS));
}

#[test]
fn correct_and_incorrect_files() {
    let run = check_files(&[
        ("first.rs", GENERAL_CORRECT),
        ("second.rs", GENERAL_INCORRECT_1),
    ]);
    assert_eq!(run.status.code(), Some(FAILURE_STATUS));
}

#[test]
fn incorrect_and_correct_files() {
    let run = check_files(&[
        ("first.rs", GENERAL_INCORRECT_2),
        ("second.rs", GENERAL_CORRECT),
    ]);
    assert_eq!(run.status.code(), Some(FAILURE_STATUS));
}
