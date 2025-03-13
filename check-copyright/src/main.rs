// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::path::PathBuf;
use std::process::exit;
use std::{fs, io};

use clap::Parser;
use regex::Regex;

const GENERAL_PATTERN: &str = r"Copyright \(c\) 202[0-9] Graphcore Ltd\. All rights reserved\.";
const LICENSE_PATTERN: &str = r"Copyright \(c\) 202[0-9] Graphcore Ltd\.\n";

const LICENSE_FILENAME: &str = "LICENSE";

const FAILURE_STATUS: i32 = 1;

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Check files contains the correct copyright notice")]
struct Cli {
    /// Paths to the files to be checked
    #[clap(required = true)]
    files: Vec<String>,
}

fn main() -> io::Result<()> {
    let args = Cli::parse();
    let gen_re = Regex::new(GENERAL_PATTERN).expect("`GENERAL_PATTERN` should be a valid regex");
    let lic_re = Regex::new(LICENSE_PATTERN).expect("`LICENSE_PATTERN` should be a valid regex");

    let mut re: &Regex;
    let mut failures = Vec::new();

    for filename in args.files {
        let content = fs::read_to_string(&filename)?;
        let abs_path = PathBuf::from(filename).canonicalize()?;

        if abs_path.file_name().unwrap() == LICENSE_FILENAME {
            re = &lic_re;
        } else {
            re = &gen_re;
        }

        if !re.is_match(&content) {
            failures.push(format!(
                "No valid copyright notice found in {}",
                abs_path.display()
            ));
        }
    }

    if !failures.is_empty() {
        let err_msg = failures
            .iter()
            .map(String::from)
            .reduce(|acc, e| format!("{acc}\n{e}"))
            .unwrap();
        println!("{err_msg}");
        exit(FAILURE_STATUS);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::Command;
    use std::str;
    use std::sync::Once;

    use tempfile::{Builder, NamedTempFile, tempdir};

    use super::*;

    const SUCCESS_STATUS: i32 = 0;

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

    static BUILD: Once = Once::new();

    fn build_binary() {
        BUILD.call_once(|| {
            let run = Command::new("cargo")
                .args(["build", "-p", "check-copyright"])
                .status()
                .expect("failed to execute process");
            assert!(run.success());
        });
    }

    fn get_workspace_dir() -> PathBuf {
        let package_dir = std::env::current_dir().unwrap();
        package_dir.parent().unwrap().to_path_buf()
    }

    #[test]
    fn no_arg() {
        build_binary();
        let run = Command::new("./target/debug/check-copyright")
            .current_dir(get_workspace_dir())
            .output()
            .expect("failed to execute process");

        assert_ne!(
            run.status.code().expect("exit code not signal"),
            SUCCESS_STATUS
        );
    }

    #[test]
    fn invalid_filename() {
        build_binary();
        let run = Command::new("./target/debug/check-copyright")
            .current_dir(get_workspace_dir())
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
        build_binary();
        let run = Command::new("./target/debug/check-copyright")
            .current_dir(get_workspace_dir())
            .arg(".")
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

        build_binary();
        let run = Command::new("./target/debug/check-copyright")
            .current_dir(get_workspace_dir())
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

        build_binary();
        let run = Command::new("./target/debug/check-copyright")
            .current_dir(get_workspace_dir())
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

        build_binary();
        let run = Command::new("./target/debug/check-copyright")
            .current_dir(get_workspace_dir())
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

        build_binary();
        let run = Command::new("./target/debug/check-copyright")
            .current_dir(get_workspace_dir())
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

        build_binary();
        let run = Command::new("./target/debug/check-copyright")
            .current_dir(get_workspace_dir())
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

        build_binary();
        let run = Command::new("./target/debug/check-copyright")
            .current_dir(get_workspace_dir())
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

        build_binary();
        let run = Command::new("./target/debug/check-copyright")
            .current_dir(get_workspace_dir())
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
        writeln!(file_b, "{GENERAL_INCORRECT_1}")
            .expect("test should be able to write to tempfile");

        build_binary();
        let run = Command::new("./target/debug/check-copyright")
            .current_dir(get_workspace_dir())
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
        writeln!(file_a, "{GENERAL_INCORRECT_2}")
            .expect("test should be able to write to tempfile");

        let mut file_b = NamedTempFile::new().expect("test should be able to create a tempfile");
        writeln!(file_b, "{GENERAL_CORRECT}").expect("test should be able to write to tempfile");

        build_binary();
        let run = Command::new("./target/debug/check-copyright")
            .current_dir(get_workspace_dir())
            .args([file_a.path(), file_b.path()])
            .output()
            .expect("failed to execute process");

        assert_eq!(
            run.status.code().expect("exit code not signal"),
            FAILURE_STATUS
        );
    }
}
