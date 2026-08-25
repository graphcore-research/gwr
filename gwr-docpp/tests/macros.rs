// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

const COMMAND_OUTPUT: &str = gwr_docpp::cmd!("printf docpp-command-output");
const MULTIPLE_COMMANDS: &str = gwr_docpp::cmd!("printf first; printf second");
const COMMAND_STREAMS: &str = gwr_docpp::cmd!("sh gwr-docpp/tests/fixtures/command_output.sh");
const INCLUDED: &str = gwr_docpp::include_str!("gwr-docpp/tests/fixtures/included.txt");
const SECTION: &str = gwr_docpp::section!(title = "Section title", text = "Section body",);
const REORDERED_SECTION: &str =
    gwr_docpp::section!(text = "Reordered body", title = "Reordered title");
const TOC: &str = gwr_docpp::toc!(
    = "Guide"
    [
        + "API"
        [
            + "Models"
            [
                - "Widget", "crate::Widget"
            ]
        ]
        - "Overview", "self"
    ]
);

#[cfg(feature = "asciidoctor")]
const ASCIIDOC: &str = gwr_docpp::adoc!("This is [big]#important#");
#[cfg(feature = "typst")]
const TYPST: &str =
    gwr_docpp::typst!(r#"See #link("https://example.com/api")[the API reference]."#);

#[test]
fn captures_command_output() {
    assert_eq!(COMMAND_OUTPUT, "docpp-command-output");
}

#[test]
fn runs_multiple_commands_in_sequence() {
    assert_eq!(MULTIPLE_COMMANDS, "firstsecond");
}

#[test]
fn captures_command_stdout_before_stderr() {
    assert_eq!(COMMAND_STREAMS, "standard output\nstandard error\n");
}

#[test]
fn includes_workspace_relative_file() {
    assert_eq!(
        INCLUDED,
        "Copyright (c) 2026 Graphcore Ltd. All rights reserved.\n\nIncluded documentation.\nSecond line.\n"
    );
}

#[test]
fn renders_section_as_markdown() {
    assert_eq!(SECTION, "# Section title\n\nSection body\n\n");
    assert_eq!(REORDERED_SECTION, "# Reordered title\n\nReordered body\n\n");
}

#[test]
fn renders_nested_table_of_contents_as_markdown() {
    assert_eq!(
        TOC,
        "# Guide\n\n## API\n\n### Models\n\n[Widget](crate::Widget)\n\n[Overview](self)\n\n"
    );
}

#[cfg(feature = "asciidoctor")]
#[test]
fn renders_asciidoc_as_html() {
    assert!(ASCIIDOC.contains("<span class=\"big\">important</span>"));
}

#[cfg(feature = "typst")]
#[test]
fn renders_typst_as_svg() {
    assert!(TYPST.starts_with("<svg "));
    assert!(TYPST.ends_with("</svg>"));
    assert!(TYPST.contains("href=\"https://example.com/api\""));
}
