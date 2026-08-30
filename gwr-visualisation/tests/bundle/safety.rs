// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::Path;

use super::common::{SMALL_TIMETABLE, generator_command};

#[test]
fn rejects_output_that_is_the_timetable_input() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let timetable = output_dir.join("data.json");
    let contents = std::fs::read_to_string(SMALL_TIMETABLE).unwrap();
    std::fs::write(&timetable, &contents).unwrap();

    assert_input_alias_is_rejected(&timetable, &output_dir, &timetable, &contents);
}

#[test]
fn rejects_output_hard_linked_to_an_input() {
    let temp = tempfile::tempdir().unwrap();
    let timetable = temp.path().join("timetable.yaml");
    let contents = std::fs::read_to_string(SMALL_TIMETABLE).unwrap();
    std::fs::write(&timetable, &contents).unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let output_path = output_dir.join("data.json");
    std::fs::hard_link(&timetable, &output_path).unwrap();

    assert_input_alias_is_rejected(&timetable, &output_dir, &output_path, &contents);
}

#[cfg(unix)]
#[test]
fn rejects_output_symlinked_to_an_input() {
    let temp = tempfile::tempdir().unwrap();
    let timetable = temp.path().join("timetable.yaml");
    let contents = std::fs::read_to_string(SMALL_TIMETABLE).unwrap();
    std::fs::write(&timetable, &contents).unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let output_path = output_dir.join("data.json");
    std::os::unix::fs::symlink(&timetable, &output_path).unwrap();

    let output = generator_command()
        .arg("--timetable")
        .arg(&timetable)
        .arg("--out")
        .arg(&output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(std::fs::read_to_string(&timetable).unwrap(), contents);
    assert_eq!(std::fs::read_link(&output_path).unwrap(), timetable);
    assert!(String::from_utf8_lossy(&output.stderr).contains(&format!(
        "Output file '{}' is a symbolic link",
        output_path.display()
    )));
}

#[test]
fn rejects_hard_links_between_output_files() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let first_output = output_dir.join("index.html");
    let second_output = output_dir.join("data.js");
    let contents = "existing report";
    std::fs::write(&first_output, contents).unwrap();
    std::fs::hard_link(&first_output, &second_output).unwrap();

    assert_output_alias_is_rejected(&output_dir, &first_output, &second_output, contents);
}

#[cfg(unix)]
#[test]
fn rejects_symlinks_between_output_files() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let first_output = output_dir.join("index.html");
    let second_output = output_dir.join("data.js");
    let contents = "existing report";
    std::fs::write(&first_output, contents).unwrap();
    std::os::unix::fs::symlink(&first_output, &second_output).unwrap();

    let output = generator_command()
        .arg("--timetable")
        .arg(SMALL_TIMETABLE)
        .arg("--out")
        .arg(&output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(std::fs::read_to_string(&first_output).unwrap(), contents);
    assert_eq!(std::fs::read_link(&second_output).unwrap(), first_output);
    assert!(String::from_utf8_lossy(&output.stderr).contains(&format!(
        "Output file '{}' is a symbolic link",
        second_output.display()
    )));
}

#[test]
fn replaces_an_unrelated_hard_link_without_changing_its_other_name() {
    let temp = tempfile::tempdir().unwrap();
    let external = temp.path().join("external.json");
    let contents = "external contents";
    std::fs::write(&external, contents).unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let output_path = output_dir.join("data.json");
    std::fs::hard_link(&external, &output_path).unwrap();

    let output = generator_command()
        .arg("--timetable")
        .arg(SMALL_TIMETABLE)
        .arg("--out")
        .arg(&output_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "gwr-visualisation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_to_string(&external).unwrap(), contents);
    assert!(!same_file::is_same_file(&external, &output_path).unwrap());
}

#[cfg(unix)]
#[test]
fn rejects_output_symlinked_to_an_unrelated_file() {
    let temp = tempfile::tempdir().unwrap();
    let external = temp.path().join("external.html");
    let contents = "external contents";
    std::fs::write(&external, contents).unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let output_path = output_dir.join("index.html");
    std::os::unix::fs::symlink(&external, &output_path).unwrap();

    let output = generator_command()
        .arg("--timetable")
        .arg(SMALL_TIMETABLE)
        .arg("--out")
        .arg(&output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(std::fs::read_to_string(&external).unwrap(), contents);
    assert_eq!(std::fs::read_link(&output_path).unwrap(), external);
    assert!(String::from_utf8_lossy(&output.stderr).contains(&format!(
        "Output file '{}' is a symbolic link",
        output_path.display()
    )));
}

#[cfg(unix)]
#[test]
fn rejects_a_dangling_output_symlink_before_writing() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let first_output = output_dir.join("index.html");
    let second_output = output_dir.join("data.js");
    std::os::unix::fs::symlink("data.js", &first_output).unwrap();
    let existing_output = output_dir.join("style.css");
    let existing_contents = "existing report";
    std::fs::write(&existing_output, existing_contents).unwrap();

    let output = generator_command()
        .arg("--timetable")
        .arg(SMALL_TIMETABLE)
        .arg("--out")
        .arg(&output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_link(&first_output).unwrap(),
        Path::new("data.js")
    );
    assert!(!second_output.exists());
    assert_eq!(
        std::fs::read_to_string(existing_output).unwrap(),
        existing_contents
    );
    assert!(!output_dir.join("data.json").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("InvalidInput"));
    assert!(stderr.contains(&format!(
        "Output file '{}' is a symbolic link",
        first_output.display()
    )));
}

fn assert_input_alias_is_rejected(
    timetable: &Path,
    output_dir: &Path,
    output_path: &Path,
    contents: &str,
) {
    let output = generator_command()
        .arg("--timetable")
        .arg(timetable)
        .arg("--out")
        .arg(output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(std::fs::read_to_string(timetable).unwrap(), contents);
    assert!(!output_dir.join("index.html").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("InvalidInput"));
    assert!(stderr.contains(&format!(
        "Output file '{}' aliases input file '{}'",
        output_path.display(),
        timetable.display()
    )));
}

fn assert_output_alias_is_rejected(
    output_dir: &Path,
    first_output: &Path,
    second_output: &Path,
    contents: &str,
) {
    let output = generator_command()
        .arg("--timetable")
        .arg(SMALL_TIMETABLE)
        .arg("--out")
        .arg(output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(std::fs::read_to_string(first_output).unwrap(), contents);
    assert_eq!(std::fs::read_to_string(second_output).unwrap(), contents);
    assert!(!output_dir.join("data.json").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("InvalidInput"));
    assert!(stderr.contains(&format!(
        "Output file '{}' aliases output file '{}'",
        second_output.display(),
        first_output.display()
    )));
}
