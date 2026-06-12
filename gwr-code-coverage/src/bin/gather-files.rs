// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::{error, fs};

use clap::Parser;
use gwr_code_coverage::{CoverageReport, common_directory_prefix, sanitized_components};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(about = "Snapshot source files referenced by an llvm-cov JSON report")]
struct Args {
    /// llvm-cov JSON report to read. This can be summary.json or details.json.
    report: PathBuf,

    /// Directory to copy the source files into.
    output_dir: PathBuf,
}

#[derive(Debug, Serialize)]
struct Manifest {
    coverage_report: String,
    snapshot_dir: String,
    files: Vec<ManifestFile>,
    missing: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ManifestFile {
    filename: String,
    snapshot: String,
}

fn main() -> Result<(), Box<dyn error::Error>> {
    let args = Args::parse();
    let report = CoverageReport::from_path(&args.report)?;
    let filenames = report.filenames().map(ToString::to_string).collect();
    let manifest = gather_files(&args.report, &args.output_dir, filenames)?;

    println!(
        "Copied {} source files into {}",
        manifest.files.len(),
        args.output_dir.display()
    );
    if !manifest.missing.is_empty() {
        println!(
            "Skipped {} source files that were not readable",
            manifest.missing.len()
        );
    }

    Ok(())
}

fn gather_files(
    report_path: &Path,
    output_dir: &Path,
    filenames: BTreeSet<String>,
) -> Result<Manifest, Box<dyn error::Error>> {
    let mut files = Vec::new();
    let mut missing = Vec::new();
    let common_prefix = common_directory_prefix(filenames.iter().map(Path::new));

    fs::create_dir_all(output_dir)?;
    for filename in filenames {
        let source_path = Path::new(&filename);
        let Some(relative_path) = snapshot_relative_path(source_path, &common_prefix) else {
            missing.push(filename);
            continue;
        };
        let snapshot_path = output_dir.join(&relative_path);

        if source_path.is_file() {
            if let Some(parent) = snapshot_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(source_path, &snapshot_path)?;
            files.push(ManifestFile {
                filename,
                snapshot: relative_path.to_string_lossy().into_owned(),
            });
        } else {
            missing.push(filename);
        }
    }

    let manifest = Manifest {
        coverage_report: report_path.display().to_string(),
        snapshot_dir: output_dir.display().to_string(),
        files,
        missing,
    };
    let manifest_path = output_dir.join("manifest.json");
    fs::write(
        manifest_path,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;

    Ok(manifest)
}

fn snapshot_relative_path(path: &Path, common_prefix: &[OsString]) -> Option<PathBuf> {
    let mut components = sanitized_components(path);
    if components.starts_with(common_prefix) {
        components.drain(..common_prefix.len());
    }

    let mut relative_path = PathBuf::new();
    for component in components {
        relative_path.push(component);
    }

    (!relative_path.as_os_str().is_empty()).then_some(relative_path)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{common_directory_prefix, gather_files, snapshot_relative_path};

    #[test]
    fn gather_files_copies_sources_after_stripping_common_prefix() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let project_dir = temp_dir.path().join("project");
        let output_dir = temp_dir.path().join("snapshot");
        let lib_rs = project_dir.join("crate-a/src/lib.rs");
        let main_rs = project_dir.join("crate-b/src/main.rs");
        write_source(&lib_rs, "pub fn lib() {}\n");
        write_source(&main_rs, "fn main() {}\n");

        let manifest = gather_files(
            &temp_dir.path().join("details.json"),
            &output_dir,
            BTreeSet::from([lib_rs.display().to_string(), main_rs.display().to_string()]),
        )
        .unwrap();

        assert!(output_dir.join("crate-a/src/lib.rs").is_file());
        assert!(output_dir.join("crate-b/src/main.rs").is_file());
        assert_eq!(
            manifest
                .files
                .iter()
                .map(|file| file.snapshot.as_str())
                .collect::<Vec<_>>(),
            vec!["crate-a/src/lib.rs", "crate-b/src/main.rs"]
        );
        assert!(manifest.missing.is_empty());
        assert!(
            fs::read_to_string(output_dir.join("manifest.json"))
                .unwrap()
                .contains(r#""snapshot": "crate-a/src/lib.rs""#)
        );
    }

    #[test]
    fn snapshot_path_strips_common_prefix() {
        let common_prefix = common_directory_prefix(
            [
                Path::new("/tmp/project/src/lib.rs"),
                Path::new("/tmp/project/tests/test.rs"),
            ]
            .into_iter(),
        );

        assert_eq!(
            snapshot_relative_path(Path::new("/tmp/project/src/lib.rs"), &common_prefix),
            Some(PathBuf::from("src/lib.rs"))
        );
        assert_eq!(
            snapshot_relative_path(Path::new("/tmp/project/tests/test.rs"), &common_prefix),
            Some(PathBuf::from("tests/test.rs"))
        );
    }

    #[test]
    fn snapshot_path_for_single_file_strips_containing_directory() {
        let common_prefix =
            common_directory_prefix([Path::new("/tmp/project/src/lib.rs")].into_iter());

        assert_eq!(
            snapshot_relative_path(Path::new("/tmp/project/src/lib.rs"), &common_prefix),
            Some(PathBuf::from("lib.rs"))
        );
    }

    #[test]
    fn snapshot_path_ignores_parent_components() {
        let common_prefix = Vec::new();

        assert_eq!(
            snapshot_relative_path(Path::new("../src/lib.rs"), &common_prefix),
            Some(PathBuf::from("src/lib.rs"))
        );
    }

    fn write_source(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}
