// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Shared helper functions for GWR `build.rs` scripts.
//!
//! Other libraries need to add this line to their top-level `lib.rs`
//! to include the full contents of the `README.md`:
//!
//! #!\[doc = include_str!(gwr_build::generated_crate_docs_path!())\]
//!
//! And then add the following to the \[dependencies\] and
//! \[build-dependencies\] in ther `Cargo.toml`:
//! gwr-build = { path = "../gwr-build", version = "X.Y.Z" }

use std::path::{Path, PathBuf};
use std::{env, fs};

/// File name for generated pre-processed crate markdown file. Must match name
/// used by generated_crate_docs_path!().
const CRATE_DOCS_MD: &str = "crate-docs.md";

#[macro_export]
macro_rules! generated_crate_docs_path {
    () => {
        concat!(env!("OUT_DIR"), "/crate-docs.md")
    };
}

#[derive(Debug, PartialEq, Eq)]
pub struct IncludeDirective<'a> {
    pub path: &'a str,
    pub anchor: Option<&'a str>,
}

pub fn write_expanded_readme_docs() {
    let manifest_dir = manifest_dir();
    let readme_path = manifest_dir.join("README.md");
    let expanded = expand_mdbook_includes(&readme_path);

    let output_path = out_dir().join(CRATE_DOCS_MD);
    fs::write(&output_path, expanded)
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", output_path.display()));
}

#[must_use]
pub fn out_dir() -> PathBuf {
    PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"))
}

#[must_use]
pub fn manifest_dir() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("Cargo should set CARGO_MANIFEST_DIR during build"),
    )
}

#[must_use]
pub fn expand_mdbook_includes(markdown_path: &Path) -> String {
    println!("cargo:rerun-if-changed={}", markdown_path.display());

    let markdown = fs::read_to_string(markdown_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", markdown_path.display()));
    let base_dir = markdown_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    let mut expanded = String::new();
    for line in markdown.lines() {
        if let Some(include) = parse_mdbook_include_directive(line) {
            let include_path = base_dir.join(include.path);
            println!("cargo:rerun-if-changed={}", include_path.display());
            let include_contents = fs::read_to_string(&include_path)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", include_path.display()));
            let include_contents = match include.anchor {
                Some(anchor) => extract_anchor_contents(&include_contents, anchor)
                    .unwrap_or_else(|err| panic!("{}: {err}", include_path.display())),
                None => include_contents,
            };
            expanded.push_str(&include_contents);
            if !include_contents.ends_with('\n') {
                expanded.push('\n');
            }
        } else {
            expanded.push_str(line);
            expanded.push('\n');
        }
    }

    expanded
}

#[must_use]
pub fn parse_mdbook_include_directive(line: &str) -> Option<IncludeDirective<'_>> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("{{#include ")?;
    let spec = rest.strip_suffix("}}")?.trim();
    let (path, anchor) = match spec.split_once(':') {
        Some((path, anchor)) => (path.trim(), Some(anchor.trim())),
        None => (spec, None),
    };
    Some(IncludeDirective { path, anchor })
}

pub fn extract_anchor_contents(contents: &str, anchor: &str) -> Result<String, String> {
    let start_marker = format!("ANCHOR: {anchor}");
    let end_marker = format!("ANCHOR_END: {anchor}");
    let mut in_anchor = false;
    let mut found_start = false;
    let mut found_end = false;
    let mut extracted = String::new();

    for line in contents.lines() {
        if line.contains(&start_marker) {
            if found_start {
                return Err(format!("duplicate start marker for anchor '{anchor}'"));
            }
            found_start = true;
            in_anchor = true;
            continue;
        }

        if line.contains(&end_marker) {
            if !in_anchor {
                return Err(format!(
                    "end marker found before start marker for anchor '{anchor}'"
                ));
            }
            found_end = true;
            break;
        }

        if in_anchor {
            extracted.push_str(line);
            extracted.push('\n');
        }
    }

    if !found_start {
        return Err(format!("missing start marker for anchor '{anchor}'"));
    }

    if !found_end {
        return Err(format!("missing end marker for anchor '{anchor}'"));
    }

    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use super::{IncludeDirective, extract_anchor_contents, parse_mdbook_include_directive};

    #[test]
    fn parses_basic_include_directive() {
        assert_eq!(
            parse_mdbook_include_directive("{{#include ./examples/simple.yaml}}"),
            Some(IncludeDirective {
                path: "./examples/simple.yaml",
                anchor: None,
            })
        );
    }

    #[test]
    fn parses_anchored_include_directive() {
        assert_eq!(
            parse_mdbook_include_directive("{{#include ../../../README.md:intro}}"),
            Some(IncludeDirective {
                path: "../../../README.md",
                anchor: Some("intro"),
            })
        );
    }

    #[test]
    fn extracts_markdown_anchor_contents() {
        let contents = "\
before
<!-- ANCHOR: overview -->
line one
line two
<!-- ANCHOR_END: overview -->
after
";

        assert_eq!(
            extract_anchor_contents(contents, "overview").unwrap(),
            "line one\nline two\n"
        );
    }

    #[test]
    fn extracts_code_anchor_contents() {
        let contents = "\
// ANCHOR: use
use std::path::Path;
// ANCHOR_END: use
";

        assert_eq!(
            extract_anchor_contents(contents, "use").unwrap(),
            "use std::path::Path;\n"
        );
    }

    #[test]
    fn errors_when_anchor_missing() {
        let err = extract_anchor_contents("no anchors here\n", "overview").unwrap_err();
        assert!(err.contains("missing start marker"));
    }

    #[test]
    fn errors_when_anchor_end_missing() {
        let err = extract_anchor_contents("<!-- ANCHOR: overview -->\nline one\n", "overview")
            .unwrap_err();
        assert!(err.contains("missing end marker"));
    }

    #[test]
    fn errors_on_duplicate_start_marker() {
        let contents = "\
<!-- ANCHOR: overview -->
line one
<!-- ANCHOR: overview -->
line two
<!-- ANCHOR_END: overview -->
";

        let err = extract_anchor_contents(contents, "overview").unwrap_err();
        assert!(err.contains("duplicate start marker"));
    }

    #[test]
    fn errors_on_end_before_start_marker() {
        let contents = "\
<!-- ANCHOR_END: overview -->
<!-- ANCHOR: overview -->
line one
";

        let err = extract_anchor_contents(contents, "overview").unwrap_err();
        assert!(err.contains("end marker found before start marker"));
    }

    #[test]
    fn trims_whitespace_in_anchored_include_directive() {
        assert_eq!(
            parse_mdbook_include_directive("  {{#include ../README.md:overview}}  "),
            Some(IncludeDirective {
                path: "../README.md",
                anchor: Some("overview"),
            })
        );
    }
}
