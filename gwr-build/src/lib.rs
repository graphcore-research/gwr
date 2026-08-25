// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#![doc = std::include_str!("../README.md")]

use std::path::{Path, PathBuf};
use std::{env, fs};

/// File name for generated pre-processed crate markdown file.
const CRATE_DOCS_MD: &str = "crate-docs.md";

#[derive(Debug, PartialEq, Eq)]
pub struct IncludeDirective<'a> {
    pub path: &'a str,
    pub anchor: Option<&'a str>,
}

pub fn write_expanded_readme_docs() {
    let manifest_dir = manifest_dir();
    let readme_path = manifest_dir.join("README.md");
    let expanded = prepare_crate_docs(&readme_path);

    let output_path = out_dir().join(CRATE_DOCS_MD);
    fs::write(&output_path, expanded)
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", output_path.display()));
}

#[must_use]
fn prepare_crate_docs(markdown_path: &Path) -> String {
    let markdown = read_markdown_file(markdown_path);
    let base_dir = markdown_path.parent().unwrap_or_else(|| Path::new("."));
    let markdown = expand_mdbook_includes(&markdown, base_dir);
    rewrite_rustdoc_links(&markdown)
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

fn read_markdown_file(markdown_path: &Path) -> String {
    println!("cargo:rerun-if-changed={}", markdown_path.display());

    fs::read_to_string(markdown_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", markdown_path.display()))
}

#[must_use]
pub fn expand_mdbook_includes(markdown: &str, base_dir: &Path) -> String {
    let mut expanded = String::new();
    for line in markdown.lines() {
        if let Some(include) = parse_mdbook_include_directive(line) {
            let include_path = base_dir.join(include.path);
            let include_contents = read_markdown_file(&include_path);
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
pub fn rewrite_rustdoc_links(markdown: &str) -> String {
    let mut rewritten = String::new();
    let mut previous_line_was_empty_reference_definition = false;

    for line in markdown.lines() {
        if previous_line_was_empty_reference_definition && is_indented_line(line) {
            rewritten.push_str(&rewrite_indented_reference_definition_target(line));
            rewritten.push('\n');
            previous_line_was_empty_reference_definition = false;
            continue;
        }

        let rewritten_line = rewrite_rustdoc_links_in_line(line);
        previous_line_was_empty_reference_definition =
            is_empty_reference_definition(&rewritten_line);
        rewritten.push_str(&rewritten_line);
        rewritten.push('\n');
    }

    rewritten
}

fn rewrite_rustdoc_links_in_line(line: &str) -> String {
    let line = rewrite_reference_definition_link(line);

    let mut rewritten = String::new();
    let mut rest = line.as_str();
    while let Some(start) = rest.find("](") {
        let link_start = start + 2;
        rewritten.push_str(&rest[..link_start]);
        rest = &rest[link_start..];

        let Some(end) = rest.find(')') else {
            rewritten.push_str(rest);
            return rewritten;
        };

        let target = &rest[..end];
        rewritten.push_str(&rewrite_link_destination(target));
        rewritten.push(')');
        rest = &rest[end + 1..];
    }
    rewritten.push_str(rest);

    rewritten
}

fn rewrite_reference_definition_link(line: &str) -> String {
    let Some((label, target)) = line.split_once("]:") else {
        return line.to_string();
    };

    if !label.starts_with('[') {
        return line.to_string();
    }

    let leading_whitespace = target.len() - target.trim_start().len();
    let (whitespace, target) = target.split_at(leading_whitespace);
    let target_end = target.find(char::is_whitespace).unwrap_or(target.len());
    let (target, suffix) = target.split_at(target_end);

    format!(
        "{label}]:{whitespace}{}{}",
        rewrite_link_destination(target),
        suffix
    )
}

fn is_empty_reference_definition(line: &str) -> bool {
    let Some((label, target)) = line.split_once("]:") else {
        return false;
    };

    label.starts_with('[') && target.trim().is_empty()
}

fn is_indented_line(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

fn rewrite_indented_reference_definition_target(line: &str) -> String {
    let leading_whitespace = line.len() - line.trim_start().len();
    let (whitespace, target) = line.split_at(leading_whitespace);
    let target_end = target.find(char::is_whitespace).unwrap_or(target.len());
    let (target, suffix) = target.split_at(target_end);

    format!("{whitespace}{}{}", rewrite_link_destination(target), suffix)
}

fn rewrite_link_destination(target: &str) -> String {
    let (target, fragment) = target
        .split_once('#')
        .map_or((target, ""), |(target, fragment)| (target, fragment));
    let fragment = if fragment.is_empty() {
        String::new()
    } else {
        format!("#{fragment}")
    };

    if let Some(source_link) = rewrite_rust_source_link_destination(target, &fragment) {
        return source_link;
    }

    if let Some(page) = target
        .strip_prefix("../gwr-developer-guide/md_src/")
        .and_then(|path| path.strip_suffix(".md"))
    {
        return format!("../../html/{page}.html{fragment}");
    }

    if let Some(crate_name) = target
        .strip_prefix("../")
        .and_then(|path| path.strip_suffix("/README.md"))
        .filter(|path| path.starts_with("gwr-"))
    {
        return format!("../{}/index.html{fragment}", crate_name.replace('-', "_"));
    }

    format!("{target}{fragment}")
}

fn rewrite_rust_source_link_destination(target: &str, fragment: &str) -> Option<String> {
    if let Some(module_path) = target
        .strip_prefix("src/")
        .and_then(rust_source_module_path)
    {
        return Some(format!("{}{}", rustdoc_index_path(&module_path), fragment));
    }

    let (crate_name, source_path) = target.strip_prefix("../")?.split_once("/src/")?;
    if !crate_name.starts_with("gwr-") {
        return None;
    }

    let module_path = rust_source_module_path(source_path)?;
    Some(format!(
        "../{}/{}{}",
        crate_name.replace('-', "_"),
        rustdoc_index_path(&module_path),
        fragment
    ))
}

fn rust_source_module_path(path: &str) -> Option<String> {
    let path = path.strip_suffix(".rs")?;
    Some(match path {
        "lib" | "main" => String::new(),
        path => path.strip_suffix("/mod").unwrap_or(path).to_string(),
    })
}

fn rustdoc_index_path(module_path: &str) -> String {
    if module_path.is_empty() {
        "index.html".to_string()
    } else {
        format!("{module_path}/index.html")
    }
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
    use std::{env, fs};

    use super::{
        IncludeDirective, expand_mdbook_includes, extract_anchor_contents,
        parse_mdbook_include_directive, prepare_crate_docs, rewrite_rustdoc_links,
    };

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

    #[test]
    fn expands_mdbook_include_directives_relative_to_base_dir() {
        let base_dir = env::temp_dir().join(format!(
            "gwr-build-expand-mdbook-includes-{}",
            std::process::id()
        ));
        fs::create_dir_all(&base_dir).unwrap();
        fs::write(
            base_dir.join("include.md"),
            "\
before
<!-- ANCHOR: wanted -->
included
<!-- ANCHOR_END: wanted -->
after
",
        )
        .unwrap();

        assert_eq!(
            expand_mdbook_includes("start\n{{#include include.md:wanted}}\nend\n", &base_dir,),
            "start\nincluded\nend\n",
        );

        fs::remove_dir_all(base_dir).unwrap();
    }

    #[test]
    fn rewrites_repository_readme_links_to_rustdoc_crate_indexes() {
        assert_eq!(
            rewrite_rustdoc_links("[components]: ../gwr-components/README.md\n"),
            "[components]: ../gwr_components/index.html\n"
        );
    }

    #[test]
    fn rewrites_repository_readme_links_with_fragments_to_rustdoc_crate_indexes() {
        assert_eq!(
            rewrite_rustdoc_links(
                "See the [flow controlled pipeline](../gwr-models/README.md#flow-controlled-pipeline).\n",
            ),
            "See the [flow controlled pipeline](../gwr_models/index.html#flow-controlled-pipeline).\n",
        );
    }

    #[test]
    fn rewrites_developer_guide_source_links_to_rendered_book_pages() {
        assert_eq!(
            rewrite_rustdoc_links(
                "[input port]: ../gwr-developer-guide/md_src/components/ports.md#input-ports\n",
            ),
            "[input port]: ../../html/components/ports.html#input-ports\n",
        );
    }

    #[test]
    fn rewrites_multiline_developer_guide_source_links_to_rendered_book_pages() {
        assert_eq!(
            rewrite_rustdoc_links(
                "[components]:\n  ../gwr-developer-guide/md_src/components/chapter.md#creating-new-components\n",
            ),
            "[components]:\n  ../../html/components/chapter.html#creating-new-components\n",
        );
    }

    #[test]
    fn rewrites_local_rust_source_links_to_rustdoc_module_pages() {
        assert_eq!(
            rewrite_rustdoc_links("[clock]: src/time/clock.rs\n"),
            "[clock]: time/clock/index.html\n",
        );
        assert_eq!(
            rewrite_rustdoc_links("[`time`]: src/time/mod.rs\n"),
            "[`time`]: time/index.html\n",
        );
    }

    #[test]
    fn rewrites_local_rust_root_source_links_to_rustdoc_crate_index() {
        assert_eq!(
            rewrite_rustdoc_links("[crate root]: src/lib.rs#overview\n"),
            "[crate root]: index.html#overview\n",
        );
        assert_eq!(
            rewrite_rustdoc_links("[binary root]: src/main.rs\n"),
            "[binary root]: index.html\n",
        );
    }

    #[test]
    fn rewrites_sibling_crate_rust_source_links_to_rustdoc_module_pages() {
        assert_eq!(
            rewrite_rustdoc_links("[clock]: ../gwr-engine/src/time/clock.rs#module-docs\n"),
            "[clock]: ../gwr_engine/time/clock/index.html#module-docs\n",
        );
    }

    #[test]
    fn rewrites_sibling_crate_rust_root_source_links_to_rustdoc_crate_index() {
        assert_eq!(
            rewrite_rustdoc_links("[engine]: ../gwr-engine/src/lib.rs#overview\n"),
            "[engine]: ../gwr_engine/index.html#overview\n",
        );
        assert_eq!(
            rewrite_rustdoc_links("[example]: ../gwr-engine/src/main.rs\n"),
            "[example]: ../gwr_engine/index.html\n",
        );
    }

    #[test]
    fn prepares_crate_docs_by_expanding_includes_and_rewriting_links() {
        let markdown_path = env::temp_dir().join(format!(
            "gwr-build-prepare-crate-docs-{}.md",
            std::process::id()
        ));
        fs::write(
            &markdown_path,
            "[clock]: src/time/clock.rs\n\n```rust\nfoo();\n```\n",
        )
        .unwrap();

        assert_eq!(
            prepare_crate_docs(&markdown_path),
            "[clock]: time/clock/index.html\n\n```rust\nfoo();\n```\n",
        );

        fs::remove_file(markdown_path).unwrap();
    }
}
