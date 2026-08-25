// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#![doc = std::include_str!("../README.md")]

use std::path::{Component, Path, PathBuf};
use std::{env, fs};

use pulldown_cmark::{Event, Parser, Tag};

/// File name for generated pre-processed crate markdown file.
const CRATE_DOCS_MD: &str = "crate-docs.md";

#[derive(Debug, PartialEq, Eq)]
struct IncludeDirective<'a> {
    path: &'a str,
    anchor: Option<&'a str>,
}

/// Expands the crate's README includes, rewrites links for rustdoc, and writes
/// `crate-docs.md` to Cargo's `OUT_DIR`.
///
/// Uses Cargo's `CARGO_PKG_README` to resolve the manifest's `package.readme`.
/// Emits Cargo rebuild directives for the README and included files.
///
/// # Panics
///
/// Panics if Cargo's environment variables are missing, no README is
/// configured, a file cannot be read or written, or an included anchor is
/// invalid.
pub fn write_expanded_readme_docs() {
    let manifest_dir = manifest_dir();
    let readme =
        env::var("CARGO_PKG_README").expect("Cargo should set CARGO_PKG_README during build");
    assert!(!readme.is_empty(), "the package must configure a README");
    let expanded = prepare_crate_docs(&manifest_dir, Path::new(&readme));

    let output_path = out_dir().join(CRATE_DOCS_MD);
    fs::write(&output_path, expanded)
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", output_path.display()));
}

fn prepare_crate_docs(crate_dir: &Path, readme_path: &Path) -> String {
    let markdown = read_markdown_file(&crate_dir.join(readme_path));
    let readme_dir = readme_path.parent().unwrap_or_else(|| Path::new("."));
    let markdown = expand_mdbook_includes(&markdown, &crate_dir.join(readme_dir));
    rewrite_rustdoc_links(readme_path, &markdown)
}

fn out_dir() -> PathBuf {
    PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR should be set by Cargo during the build"))
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("Cargo should set CARGO_MANIFEST_DIR during build"),
    )
}

fn read_markdown_file(markdown_path: &Path) -> String {
    println!("cargo:rerun-if-changed={}", markdown_path.display());

    fs::read_to_string(markdown_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", markdown_path.display()))
}

fn expand_mdbook_includes(markdown: &str, base_dir: &Path) -> String {
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

fn rewrite_rustdoc_links(readme_path: &Path, markdown: &str) -> String {
    let parser = Parser::new(markdown);
    let mut replacements = Vec::new();
    for (_, definition) in parser.reference_definitions().iter() {
        let source = &markdown[definition.span.clone()];
        // Search after the label so a label containing the URL is preserved.
        let Some(label_end) = source.find("]:").map(|end| end + 2) else {
            continue;
        };
        let Some(offset) = source[label_end..].find(definition.dest.as_ref()) else {
            continue;
        };
        let start = definition.span.start + label_end + offset;
        replacements.push((
            start..start + definition.dest.len(),
            rewrite_link_destination(readme_path, &definition.dest),
        ));
    }
    for (event, range) in parser.into_offset_iter() {
        if matches!(event, Event::Code(_) | Event::Start(Tag::CodeBlock(_))) {
            replacements.push((range.clone(), markdown[range].to_string()));
        }
    }
    replacements.sort_by_key(|(range, _)| range.start);

    let mut rewritten = String::new();
    let mut cursor = 0;
    for (range, replacement) in replacements {
        rewritten.push_str(&rewrite_inline_links(
            readme_path,
            &markdown[cursor..range.start],
        ));
        rewritten.push_str(&replacement);
        cursor = range.end;
    }
    rewritten.push_str(&rewrite_inline_links(readme_path, &markdown[cursor..]));
    if !rewritten.is_empty() && !rewritten.ends_with('\n') {
        rewritten.push('\n');
    }
    rewritten
}

fn rewrite_inline_links(readme_path: &Path, markdown: &str) -> String {
    let mut rewritten = String::new();
    let mut rest = markdown;
    while let Some(start) = rest.find("](") {
        let link_start = start + 2;
        rewritten.push_str(&rest[..link_start]);
        rest = &rest[link_start..];

        let Some(end) = rest.find(')') else {
            rewritten.push_str(rest);
            return rewritten;
        };

        let target = &rest[..end];
        rewritten.push_str(&rewrite_link_destination_and_title(readme_path, target));
        rewritten.push(')');
        rest = &rest[end + 1..];
    }
    rewritten.push_str(rest);

    rewritten
}

fn rewrite_link_destination_and_title(readme_path: &Path, target: &str) -> String {
    if let Some((destination, title)) = target
        .strip_prefix('<')
        .and_then(|rest| rest.split_once('>'))
    {
        return format!(
            "<{}>{title}",
            rewrite_link_destination(readme_path, destination),
        );
    }

    let target_end = target.find(char::is_whitespace).unwrap_or(target.len());
    let (destination, title) = target.split_at(target_end);

    format!(
        "{}{}",
        rewrite_link_destination(readme_path, destination),
        title
    )
}

fn rewrite_link_destination(readme_path: &Path, target: &str) -> String {
    let original_target = target;
    let (target, fragment) = target.split_at(target.find('#').unwrap_or(target.len()));
    // Only repository-relative paths can use the rustdoc mappings.
    if target.is_empty() || target.starts_with(['#', '/', '?']) || target.contains(':') {
        return original_target.to_string();
    }

    let readme_dir = readme_path.parent().unwrap_or_else(|| Path::new("."));
    let normalized = normalize_link_path(&readme_dir.join(target));
    if normalized == normalize_link_path(readme_path) {
        return format!("index.html{fragment}");
    }
    let target = normalized.as_str();
    if let Some(source_link) = rewrite_rust_source_link_destination(target, fragment) {
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

    original_target.to_string()
}

// Normalize lexically: link targets need not exist on disk.
fn normalize_link_path(path: &Path) -> String {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if matches!(components.last(), Some(Component::Normal(_))) => {
                components.pop();
            }
            component => components.push(component),
        }
    }
    components
        .iter()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
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

fn parse_mdbook_include_directive(line: &str) -> Option<IncludeDirective<'_>> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("{{#include ")?;
    let spec = rest.strip_suffix("}}")?.trim();
    let (path, anchor) = match spec.split_once(':') {
        Some((path, anchor)) => (path.trim(), Some(anchor.trim())),
        None => (spec, None),
    };
    Some(IncludeDirective { path, anchor })
}

fn extract_anchor_contents(contents: &str, anchor: &str) -> Result<String, String> {
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
    use std::path::Path;
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
            rewrite_rustdoc_links(
                Path::new("README.md"),
                "[components]: ../gwr-components/README.md\n"
            ),
            "[components]: ../gwr_components/index.html\n"
        );
    }

    #[test]
    fn rewrites_repository_readme_links_with_fragments_to_rustdoc_crate_indexes() {
        assert_eq!(
            rewrite_rustdoc_links(
                Path::new("README.md"),
                "See the [flow controlled pipeline](../gwr-models/README.md#flow-controlled-pipeline).\n",
            ),
            "See the [flow controlled pipeline](../gwr_models/index.html#flow-controlled-pipeline).\n",
        );
    }

    #[test]
    fn rewrites_angle_bracket_destinations_preserving_delimiters_and_titles() {
        let cases = [
            (
                ".",
                "[engine](<../gwr-engine/README.md>)",
                "[engine](<../gwr_engine/index.html>)",
            ),
            (
                ".",
                "[engine](<../gwr-engine/README.md#overview> \"Engine docs\")",
                "[engine](<../gwr_engine/index.html#overview> \"Engine docs\")",
            ),
            (
                ".",
                "[engine]: <../gwr-engine/README.md> 'Engine docs'",
                "[engine]: <../gwr_engine/index.html> 'Engine docs'",
            ),
            (
                ".",
                "[guide]:\n  <../gwr-developer-guide/md_src/getting started.md> \"Guide\"",
                "[guide]:\n  <../../html/getting started.html> \"Guide\"",
            ),
            (
                "docs",
                "[module](<../src/lib.rs#overview>)",
                "[module](<index.html#overview>)",
            ),
            (
                "docs",
                "[external](<https://example.com/a b> \"External\")",
                "[external](<https://example.com/a b> \"External\")",
            ),
            ("docs", "[section](<#overview>)", "[section](<#overview>)"),
        ];
        for (readme_dir, markdown, expected) in cases {
            assert_eq!(
                rewrite_rustdoc_links(&Path::new(readme_dir).join("README.md"), markdown),
                format!("{expected}\n"),
                "{markdown}",
            );
        }
    }

    #[test]
    fn preserves_code_while_rewriting_surrounding_links() {
        let link = "[engine](../gwr-engine/README.md)";
        let rewritten_link = "[engine](../gwr_engine/index.html)";
        for code in [
            format!("`{link}`"),
            format!("`` `{link}` ``"),
            format!("`first line\n{link}\nlast line`"),
            "`first line\n[engine]: ../gwr-engine/README.md\nlast line`".to_string(),
            format!("```rust\nlet example = \"{link}\";\n```"),
            "~~~markdown\n[engine]: ../gwr-engine/README.md\n~~~".to_string(),
            format!("````markdown\n```rust\n{link}\n```\n````"),
            format!("    let example = \"{link}\";"),
            format!("\tlet example = \"{link}\";"),
            format!("> ```markdown\n> {link}\n> ```"),
            format!("- Example:\n\n  ```markdown\n  {link}\n  ```"),
        ] {
            let markdown = format!("{link}\n\n{code}\n\n{link}\n");
            assert_eq!(
                rewrite_rustdoc_links(Path::new("README.md"), &markdown),
                format!("{rewritten_link}\n\n{code}\n\n{rewritten_link}\n"),
                "{code}",
            );
        }
    }

    #[test]
    fn rewrites_links_beside_inline_code_and_in_code_labels() {
        let markdown =
            "`[engine](../gwr-engine/README.md)` and [`Engine`](../gwr-engine/README.md)\n";
        assert_eq!(
            rewrite_rustdoc_links(Path::new("README.md"), markdown),
            "`[engine](../gwr-engine/README.md)` and [`Engine`](../gwr_engine/index.html)\n",
        );
    }

    #[test]
    fn preserves_unclosed_fenced_code_to_end_of_document() {
        let markdown = "```markdown\n[engine](../gwr-engine/README.md)\n";
        assert_eq!(
            rewrite_rustdoc_links(Path::new("README.md"), markdown),
            markdown
        );
    }

    #[test]
    fn unmatched_backtick_does_not_hide_links() {
        assert_eq!(
            rewrite_rustdoc_links(
                Path::new("README.md"),
                "` [engine](../gwr-engine/README.md)\n"
            ),
            "` [engine](../gwr_engine/index.html)\n",
        );
    }

    #[test]
    fn rewrites_inline_links_with_multiline_titles() {
        for (destination, expected) in [
            ("../gwr-engine/README.md", "../gwr_engine/index.html"),
            (
                "<../gwr-engine/README.md#overview>",
                "<../gwr_engine/index.html#overview>",
            ),
        ] {
            for title in ["\"Engine docs\"", "'Engine docs'", "(Engine docs)"] {
                let markdown = format!("See [engine]({destination}\n  {title}) for details.\n");
                assert_eq!(
                    rewrite_rustdoc_links(Path::new("README.md"), &markdown),
                    format!("See [engine]({expected}\n  {title}) for details.\n"),
                );
            }
        }
    }

    #[test]
    fn rewrites_inline_links_without_changing_titles() {
        assert_eq!(
            rewrite_rustdoc_links(
                Path::new("README.md"),
                "Read the [engine docs](../gwr-engine/README.md \"Engine docs\").\n",
            ),
            "Read the [engine docs](../gwr_engine/index.html \"Engine docs\").\n",
        );
    }

    #[test]
    fn rewrites_indented_reference_definitions_preserving_whitespace() {
        for spaces in 1..=3 {
            let indent = " ".repeat(spaces);
            for separator in [" ", "\n  "] {
                for (destination, expected) in [
                    ("../gwr-engine/README.md", "../gwr_engine/index.html"),
                    (
                        "<../gwr-engine/README.md#overview>",
                        "<../gwr_engine/index.html#overview>",
                    ),
                ] {
                    let markdown =
                        format!("{indent}[engine]:{separator}{destination} \"Engine docs\"\n");
                    assert_eq!(
                        rewrite_rustdoc_links(Path::new("README.md"), &markdown),
                        format!("{indent}[engine]:{separator}{expected} \"Engine docs\"\n"),
                        "{markdown}",
                    );
                }
            }
        }
    }

    #[test]
    fn preserves_reference_definitions_indented_as_code() {
        for indent in ["    ", "\t"] {
            for separator in [" ", "\n    "] {
                let markdown = format!("{indent}[engine]:{separator}../gwr-engine/README.md\n");
                assert_eq!(
                    rewrite_rustdoc_links(Path::new("README.md"), &markdown),
                    markdown
                );
            }
        }
    }

    #[test]
    fn rewrites_reference_definitions_in_block_containers() {
        for markdown in [
            "> [engine]: ../gwr-engine/README.md\n",
            "> > [engine]: <../gwr-engine/README.md> \"Engine docs\"\n",
            "> [engine]:\n>   ../gwr-engine/README.md\n",
            "- [engine]: ../gwr-engine/README.md\n",
            "- Example\n\n  [engine]: ../gwr-engine/README.md\n",
        ] {
            assert_eq!(
                rewrite_rustdoc_links(Path::new("README.md"), markdown),
                markdown.replace("../gwr-engine/README.md", "../gwr_engine/index.html"),
                "{markdown}",
            );
        }
    }

    #[test]
    fn rewrites_reference_definition_links_without_changing_titles() {
        assert_eq!(
            rewrite_rustdoc_links(
                Path::new("README.md"),
                "[engine]: ../gwr-engine/README.md \"Engine docs\"\n",
            ),
            "[engine]: ../gwr_engine/index.html \"Engine docs\"\n",
        );
    }

    #[test]
    fn rewrites_multiline_reference_definition_links_without_changing_titles() {
        assert_eq!(
            rewrite_rustdoc_links(
                Path::new("README.md"),
                "[components]:\n  ../gwr-components/README.md \"Component docs\"\n",
            ),
            "[components]:\n  ../gwr_components/index.html \"Component docs\"\n",
        );
    }

    #[test]
    fn rewrites_developer_guide_source_links_to_rendered_book_pages() {
        assert_eq!(
            rewrite_rustdoc_links(
                Path::new("README.md"),
                "[input port]: ../gwr-developer-guide/md_src/components/ports.md#input-ports\n",
            ),
            "[input port]: ../../html/components/ports.html#input-ports\n",
        );
    }

    #[test]
    fn rewrites_multiline_developer_guide_source_links_to_rendered_book_pages() {
        assert_eq!(
            rewrite_rustdoc_links(
                Path::new("README.md"),
                "[components]:\n  ../gwr-developer-guide/md_src/components/chapter.md#creating-new-components\n",
            ),
            "[components]:\n  ../../html/components/chapter.html#creating-new-components\n",
        );
    }

    #[test]
    fn rewrites_local_rust_source_links_to_rustdoc_module_pages() {
        assert_eq!(
            rewrite_rustdoc_links(Path::new("README.md"), "[clock]: src/time/clock.rs\n"),
            "[clock]: time/clock/index.html\n",
        );
        assert_eq!(
            rewrite_rustdoc_links(Path::new("README.md"), "[`time`]: src/time/mod.rs\n"),
            "[`time`]: time/index.html\n",
        );
    }

    #[test]
    fn rewrites_local_rust_root_source_links_to_rustdoc_crate_index() {
        assert_eq!(
            rewrite_rustdoc_links(
                Path::new("README.md"),
                "[crate root]: src/lib.rs#overview\n"
            ),
            "[crate root]: index.html#overview\n",
        );
        assert_eq!(
            rewrite_rustdoc_links(Path::new("README.md"), "[binary root]: src/main.rs\n"),
            "[binary root]: index.html\n",
        );
    }

    #[test]
    fn rewrites_sibling_crate_rust_source_links_to_rustdoc_module_pages() {
        assert_eq!(
            rewrite_rustdoc_links(
                Path::new("README.md"),
                "[clock]: ../gwr-engine/src/time/clock.rs#module-docs\n"
            ),
            "[clock]: ../gwr_engine/time/clock/index.html#module-docs\n",
        );
    }

    #[test]
    fn rewrites_sibling_crate_rust_root_source_links_to_rustdoc_crate_index() {
        assert_eq!(
            rewrite_rustdoc_links(
                Path::new("README.md"),
                "[engine]: ../gwr-engine/src/lib.rs#overview\n"
            ),
            "[engine]: ../gwr_engine/index.html#overview\n",
        );
        assert_eq!(
            rewrite_rustdoc_links(
                Path::new("README.md"),
                "[example]: ../gwr-engine/src/main.rs\n"
            ),
            "[example]: ../gwr_engine/index.html\n",
        );
    }

    #[test]
    fn rewrites_links_relative_to_nested_readme_directory() {
        let cases = [
            ("[module](../src/lib.rs)", "[module](index.html)"),
            (
                "[clock]: ../src/./time/../time/clock.rs#overview \"Clock\"",
                "[clock]: time/clock/index.html#overview \"Clock\"",
            ),
            (
                "[engine]:\n  ../../gwr-engine/src/lib.rs#overview",
                "[engine]:\n  ../gwr_engine/index.html#overview",
            ),
            (
                "[entities](../../gwr-track/README.md#entities)",
                "[entities](../gwr_track/index.html#entities)",
            ),
            (
                "[guide](../../gwr-developer-guide/md_src/components/chapter.md)",
                "[guide](../../html/components/chapter.html)",
            ),
        ];
        for (markdown, expected) in cases {
            assert_eq!(
                rewrite_rustdoc_links(Path::new("docs/README.md"), markdown),
                format!("{expected}\n"),
                "{markdown}",
            );
        }
        assert_eq!(
            rewrite_rustdoc_links(
                Path::new("docs/reference/README.md"),
                "[module](../../src/lib.rs)"
            ),
            "[module](index.html)\n",
        );
    }

    #[test]
    fn preserves_destinations_without_rustdoc_mappings() {
        for destination in [
            "https://example.com/src/../src/lib.rs#overview",
            "mailto:docs@example.com",
            "//example.com/src/lib.rs",
            "/src/lib.rs",
            "#overview",
            "?view=source",
            "images/../diagram.svg",
        ] {
            let markdown = format!("[link]({destination})\n");
            assert_eq!(
                rewrite_rustdoc_links(Path::new("docs/README.md"), &markdown),
                markdown
            );
        }
    }

    #[test]
    fn prepares_readme_self_links_as_crate_index_links() {
        let crate_dir =
            env::temp_dir().join(format!("gwr-build-self-links-{}", std::process::id()));
        fs::create_dir_all(crate_dir.join("docs")).unwrap();
        for (readme_path, destination) in [
            ("README.md", "README.md"),
            ("README.md", "./README.md"),
            ("docs/Overview.md", "Overview.md"),
            ("docs/Overview.md", "../docs/./Overview.md"),
        ] {
            fs::write(
                crate_dir.join(readme_path),
                format!("[details]({destination}#details)\n[home](<{destination}> \"Overview\")\n\n> [details]: {destination}#details\n"),
            ).unwrap();
            assert_eq!(
                prepare_crate_docs(&crate_dir, Path::new(readme_path)),
                "[details](index.html#details)\n[home](<index.html> \"Overview\")\n\n> [details]: index.html#details\n",
                "{readme_path}: {destination}",
            );
        }
        fs::remove_dir_all(crate_dir).unwrap();
    }

    #[test]
    fn prepares_nested_readme_with_relative_includes_and_links() {
        let crate_dir =
            env::temp_dir().join(format!("gwr-build-nested-readme-{}", std::process::id()));
        fs::create_dir_all(crate_dir.join("docs")).unwrap();
        fs::write(
            crate_dir.join("docs/Overview.md"),
            "[module](../src/lib.rs)\n{{#include details.md}}\n",
        )
        .unwrap();
        fs::write(crate_dir.join("docs/details.md"), "Included details.\n").unwrap();
        assert_eq!(
            prepare_crate_docs(&crate_dir, Path::new("docs/Overview.md")),
            "[module](index.html)\nIncluded details.\n",
        );
        fs::remove_dir_all(crate_dir).unwrap();
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
            prepare_crate_docs(
                markdown_path.parent().unwrap(),
                Path::new(markdown_path.file_name().unwrap())
            ),
            "[clock]: time/clock/index.html\n\n```rust\nfoo();\n```\n",
        );

        fs::remove_file(markdown_path).unwrap();
    }
}
