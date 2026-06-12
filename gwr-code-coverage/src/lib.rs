// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fmt;

mod coverage;
mod dp_table;
mod lcs;
mod markdown;
mod paths;
mod source_files;

pub use coverage::file::coverage_did_not_decrease;
pub use coverage::report::CoverageReport;
pub use markdown::{DEFAULT_CONTEXT_LINES, RenderOptions, render_markdown_with_options};
pub use paths::{common_directory_prefix, sanitized_components};

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Json(serde_json::Error),
    EmptyReport,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(error) => write!(f, "failed to read coverage report: {error}"),
            Error::Json(error) => write!(f, "failed to parse coverage report: {error}"),
            Error::EmptyReport => write!(f, "coverage report did not contain any data"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Io(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error::Json(error)
    }
}

fn infer_prefix<'a>(filenames: impl Iterator<Item = &'a str>) -> String {
    let filenames = filenames.collect::<Vec<_>>();
    if filenames.iter().any(|filename| !filename.starts_with('/')) {
        return String::new();
    }

    let common = paths::common_directory_prefix(
        filenames
            .iter()
            .map(|filename| std::path::Path::new(*filename)),
    );
    if common.is_empty() {
        return String::new();
    }

    let prefix = common
        .iter()
        .map(|component| component.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

    normalize_prefix(&format!("/{prefix}"))
}

fn normalize_prefix(prefix: &str) -> String {
    prefix.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::infer_prefix;

    #[test]
    fn infer_prefix_uses_common_directory_prefix() {
        assert_eq!(
            infer_prefix(
                [
                    "/tmp/project/src/lib.rs",
                    "/tmp/project/tests/integration.rs"
                ]
                .into_iter()
            ),
            "/tmp/project"
        );
    }

    #[test]
    fn infer_prefix_returns_empty_when_any_filename_is_relative() {
        assert_eq!(
            infer_prefix(["/tmp/project/src/lib.rs", "src/lib.rs"].into_iter()),
            ""
        );
    }
}
