// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{error, fs, io};

use clap::{Parser, ValueEnum};
use gwr_code_coverage::{
    CoverageReport, DEFAULT_CONTEXT_LINES, RenderOptions, coverage_did_not_decrease,
    render_markdown_with_options,
};
use serde::Deserialize;

const MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Parser)]
#[command(about = "Compare two llvm-cov JSON files")]
struct Args {
    /// Baseline llvm-cov JSON file.
    before: PathBuf,

    /// Comparison llvm-cov JSON file.
    after: PathBuf,

    /// Include unchanged files in the file-level table.
    #[arg(long, value_enum, default_value_t = Files::Changed)]
    files: Files,

    /// Number of unchanged surrounding lines to show in line coverage diffs.
    #[arg(short = 'C', long, default_value_t = DEFAULT_CONTEXT_LINES)]
    context: usize,

    /// Folder prefix to remove from filenames in the baseline report.
    #[arg(long)]
    before_prefix: Option<String>,

    /// Folder prefix to remove from filenames in the comparison report.
    #[arg(long)]
    after_prefix: Option<String>,

    /// Source snapshot directory or manifest to use for the baseline report.
    #[arg(long)]
    before_snapshot: Option<String>,

    /// Source snapshot directory or manifest to use for the comparison report.
    #[arg(long)]
    after_snapshot: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Files {
    Changed,
    All,
}

fn main() -> Result<ExitCode, Box<dyn error::Error>> {
    let args = Args::parse();
    let before_path = args.before.display().to_string();
    let after_path = args.after.display().to_string();
    let before = CoverageReport::from_path(&args.before)?;
    let after = CoverageReport::from_path(&args.after)?;
    let before = apply_source_snapshot(before, args.before_snapshot.as_deref(), &args.before)?;
    let after = apply_source_snapshot(after, args.after_snapshot.as_deref(), &args.after)?;
    let show_all_files = match args.files {
        Files::Changed => false,
        Files::All => true,
    };

    print!(
        "{}",
        render_markdown_with_options(
            &before,
            &after,
            &RenderOptions {
                show_all_files,
                before_path: Some(before_path),
                after_path: Some(after_path),
                before_prefix: args.before_prefix,
                after_prefix: args.after_prefix,
                context: args.context,
            },
        )
    );

    if coverage_did_not_decrease(&before, &after) {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

fn apply_source_snapshot(
    report: CoverageReport,
    snapshot_path: Option<&str>,
    report_path: &Path,
) -> Result<CoverageReport, Box<dyn error::Error>> {
    let snapshot = SourceSnapshot::from_snapshot_or_report_path(snapshot_path, report_path)?;
    Ok(report.map_filenames(|filename| snapshot.filename(filename)))
}

#[derive(Debug, Default)]
struct SourceSnapshot {
    files: BTreeMap<String, PathBuf>,
}

impl SourceSnapshot {
    fn from_snapshot_or_report_path(
        snapshot_path: Option<&str>,
        report_path: &Path,
    ) -> Result<Self, Box<dyn error::Error>> {
        snapshot_path.map_or_else(
            || Ok(Self::from_report_path(report_path)),
            |snapshot_path| {
                Self::from_manifest_path(&snapshot_manifest_path(Path::new(snapshot_path)))
            },
        )
    }

    fn from_report_path(report_path: &Path) -> Self {
        let Some(report_dir) = report_path.parent() else {
            return Self::default();
        };
        Self::from_manifest_path(&report_dir.join("source-snapshot").join(MANIFEST_FILE))
            .unwrap_or_default()
    }

    fn from_manifest_path(manifest_path: &Path) -> Result<Self, Box<dyn error::Error>> {
        let manifest = fs::read_to_string(manifest_path).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "failed to read source snapshot manifest {}: {err}",
                    manifest_path.display()
                ),
            )
        })?;
        let manifest =
            serde_json::from_str::<SourceSnapshotManifest>(&manifest).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "failed to parse source snapshot manifest {}: {err}",
                        manifest_path.display()
                    ),
                )
            })?;
        let snapshot_dir = manifest_path
            .parent()
            .map_or_else(PathBuf::new, absolute_path);
        let files = manifest
            .files
            .into_iter()
            .map(|file| (file.filename, snapshot_dir.join(file.snapshot)))
            .collect();

        Ok(Self { files })
    }

    fn filename(&self, filename: &str) -> Option<String> {
        self.files
            .get(filename)
            .map(|snapshot_path| snapshot_path.display().to_string())
    }
}

fn snapshot_manifest_path(snapshot_path: &Path) -> PathBuf {
    if snapshot_path
        .file_name()
        .is_some_and(|filename| filename == MANIFEST_FILE)
    {
        snapshot_path.to_path_buf()
    } else {
        snapshot_path.join(MANIFEST_FILE)
    }
}

fn absolute_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[derive(Debug, Deserialize)]
struct SourceSnapshotManifest {
    files: Vec<SourceSnapshotFile>,
}

#[derive(Debug, Deserialize)]
struct SourceSnapshotFile {
    filename: String,
    snapshot: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC_FILE: &str = "src/lib.rs";

    #[test]
    fn applies_source_snapshots_next_to_json_reports_before_rendering() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let fixture = SnapshotFixture::new(
            temp_dir.path(),
            "fn first() {}\nfn inserted() {}\nfn target() {}\n",
            "fn first() {}\nfn target() {}\n",
            "fn first() {}\nfn inserted() {}\nfn target() {}\n",
        );

        let before = fixture.auto_before_report(&[[1, 1], [2, 0]], 1, 2);
        let after = fixture.auto_after_report(&[[1, 1], [2, 0], [3, 8]], 2, 3);
        let report = render_report(&before, &after);

        assert!(report.contains("+ 2 -> 3 [0 -> 8] | fn target() {}"));
        assert!(!report.contains("+ 2 -> 2 [0 -> 8]"));
    }

    #[test]
    fn applies_explicit_source_snapshot_paths_before_rendering() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let temp_dir = temp_dir.path();
        let fixture = SnapshotFixture::with_snapshot_dirs(
            temp_dir,
            temp_dir.join("snapshots/before"),
            temp_dir.join("snapshots/after"),
            "fn first() {}\nfn inserted() {}\nfn target() {}\n",
            "fn first() {}\nfn target() {}\n",
            "fn first() {}\nfn inserted() {}\nfn target() {}\n",
        );

        let before = fixture.explicit_before_report(&[[1, 1], [2, 0]], 1, 2);
        let after = fixture.explicit_after_report(&[[1, 1], [2, 0], [3, 8]], 2, 3);
        let report = render_report(&before, &after);

        assert!(report.contains("+ 2 -> 3 [0 -> 8] | fn target() {}"));
        assert!(!report.contains("+ 2 -> 2 [0 -> 8]"));
    }

    #[test]
    fn relative_report_paths_still_match_files_between_snapshots() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let fixture = SnapshotFixture::new(
            temp_dir.path(),
            "fn first() {}\nfn target() {}\n",
            "fn first() {}\nfn target() {}\n",
            "fn first() {}\nfn target() {}\n",
        );

        let before = fixture.auto_before_report(&[[1, 1], [2, 0]], 1, 2);
        let after = fixture.auto_after_report(&[[1, 1], [2, 8]], 2, 2);
        let report = render_report(&before, &after);

        assert!(report.contains("## Files: showing 1/1 changed files"));
        assert!(!report.contains("_ | _ |"));
        assert!(report.contains("+ 2 -> 2 [0 -> 8] | fn target() {}"));
    }

    #[test]
    fn explicit_source_snapshot_errors_when_manifest_is_missing() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let report = CoverageReport::from_json(&full_report_json(
            "/tmp/project/src/lib.rs",
            &[[1, 1]],
            1,
            1,
        ))
        .unwrap();
        let missing_snapshot = temp_dir.path().join("missing-snapshot");

        let err = apply_source_snapshot(
            report,
            Some(&missing_snapshot.display().to_string()),
            &temp_dir.path().join("details.json"),
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("failed to read source snapshot manifest")
        );
        assert!(err.to_string().contains(MANIFEST_FILE));
    }

    struct SnapshotFixture {
        live_source: PathBuf,
        before_report_path: PathBuf,
        after_report_path: PathBuf,
        before_snapshot_dir: PathBuf,
        after_snapshot_dir: PathBuf,
    }

    impl SnapshotFixture {
        fn new(
            temp_dir: &Path,
            live_source: &str,
            before_source: &str,
            after_source: &str,
        ) -> Self {
            Self::with_snapshot_dirs(
                temp_dir,
                temp_dir.join("before/source-snapshot"),
                temp_dir.join("after/source-snapshot"),
                live_source,
                before_source,
                after_source,
            )
        }

        fn with_snapshot_dirs(
            temp_dir: &Path,
            before_snapshot_dir: PathBuf,
            after_snapshot_dir: PathBuf,
            live_source: &str,
            before_source: &str,
            after_source: &str,
        ) -> Self {
            let fixture = Self {
                live_source: temp_dir.join("live").join(SRC_FILE),
                before_report_path: temp_dir.join("before/details.json"),
                after_report_path: temp_dir.join("after/details.json"),
                before_snapshot_dir,
                after_snapshot_dir,
            };
            fixture.write_source_files(live_source, before_source, after_source);
            fixture.write_snapshot_manifests();
            fixture
        }

        fn auto_before_report(
            &self,
            line_counts: &[[u64; 2]],
            covered: u64,
            count: u64,
        ) -> CoverageReport {
            apply_source_snapshot(
                self.report(line_counts, covered, count),
                None,
                &self.before_report_path,
            )
            .unwrap()
        }

        fn auto_after_report(
            &self,
            line_counts: &[[u64; 2]],
            covered: u64,
            count: u64,
        ) -> CoverageReport {
            apply_source_snapshot(
                self.report(line_counts, covered, count),
                None,
                &self.after_report_path,
            )
            .unwrap()
        }

        fn explicit_before_report(
            &self,
            line_counts: &[[u64; 2]],
            covered: u64,
            count: u64,
        ) -> CoverageReport {
            apply_source_snapshot(
                self.report(line_counts, covered, count),
                Some(&self.before_snapshot_dir.display().to_string()),
                &self.before_report_path,
            )
            .unwrap()
        }

        fn explicit_after_report(
            &self,
            line_counts: &[[u64; 2]],
            covered: u64,
            count: u64,
        ) -> CoverageReport {
            apply_source_snapshot(
                self.report(line_counts, covered, count),
                Some(
                    &self
                        .after_snapshot_dir
                        .join(MANIFEST_FILE)
                        .display()
                        .to_string(),
                ),
                &self.after_report_path,
            )
            .unwrap()
        }

        fn report(&self, line_counts: &[[u64; 2]], covered: u64, count: u64) -> CoverageReport {
            CoverageReport::from_json(&full_report_json(
                &self.live_source.display().to_string(),
                line_counts,
                covered,
                count,
            ))
            .unwrap()
        }

        fn write_source_files(&self, live_source: &str, before_source: &str, after_source: &str) {
            write_source(&self.live_source, live_source);
            write_source(&self.before_snapshot_dir.join(SRC_FILE), before_source);
            write_source(&self.after_snapshot_dir.join(SRC_FILE), after_source);
        }

        fn write_snapshot_manifests(&self) {
            write_snapshot_manifest(&self.before_snapshot_dir, &self.live_source);
            write_snapshot_manifest(&self.after_snapshot_dir, &self.live_source);
        }
    }

    fn write_source(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn write_snapshot_manifest(snapshot_dir: &Path, source_path: &Path) {
        fs::write(
            snapshot_dir.join(MANIFEST_FILE),
            format!(
                r#"{{
                  "coverage_report": "details.json",
                  "snapshot_dir": {snapshot_dir:?},
                  "files": [
                    {{
                      "filename": {source_path:?},
                      "snapshot": {SRC_FILE:?}
                    }}
                  ],
                  "missing": []
                }}"#
            ),
        )
        .unwrap();
    }

    fn render_report(before: &CoverageReport, after: &CoverageReport) -> String {
        render_markdown_with_options(
            before,
            after,
            &RenderOptions {
                context: 1,
                ..RenderOptions::default()
            },
        )
    }

    fn full_report_json(
        filename: &str,
        line_counts: &[[u64; 2]],
        covered: u64,
        count: u64,
    ) -> String {
        let segments = line_counts
            .iter()
            .map(|[line, count]| format!("[{line},1,{count},true,true,false]"))
            .collect::<Vec<_>>()
            .join(",");

        format!(
            r#"{{
              "data": [{{
                "files": [{{
                  "filename": {filename:?},
                  "segments": [{segments}],
                  "summary": {{
                    "lines": {{ "count": {count}, "covered": {covered}, "percent": 0.0 }},
                    "functions": {{ "count": 1, "covered": 1, "percent": 100.0 }},
                    "regions": {{ "count": {count}, "covered": {covered}, "percent": 0.0 }}
                  }}
                }}],
                "totals": {{
                  "lines": {{ "count": {count}, "covered": {covered}, "percent": 0.0 }},
                  "functions": {{ "count": 1, "covered": 1, "percent": 100.0 }},
                  "regions": {{ "count": {count}, "covered": {covered}, "percent": 0.0 }}
                }}
              }}],
              "type": "llvm.coverage.json.export",
              "version": "2.0.1"
            }}"#
        )
    }
}
