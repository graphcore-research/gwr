// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

mod common;

use common::run_builder;

#[test]
fn builds_documentation_from_cargo_expansion() {
    let run = run_builder("crate", &["--dump-all", "--verbose"]);
    run.assert_success();

    // The copyright comment is metadata for the golden fixture rather than
    // generated documentation. The repository hook keeps the fixture at one
    // trailing newline, while the builder terminates the final AsciiDoc
    // paragraph with a blank line.
    let (_, expected_documentation) = include_str!("fixtures/expected.adoc")
        .split_once("\n\n")
        .expect("golden fixture must contain a copyright header");
    let expected_documentation = format!("{expected_documentation}\n");
    assert_eq!(run.documentation, expected_documentation);

    let dump = run.stdout();
    // `--dump-all` prints `NodeType: full_name` for each parsed item followed by a
    // `doc:{...}` block containing its accumulated doc-attribute text. With
    // `GWR_DOC_BUILDER` set, `include_str!` and `typst!` expand to marker strings
    // containing their original invocations, which is why they appear here
    // verbatim.
    for expected in [
        "Module: crate",
        "Struct: crate::Widget",
        "Field: crate::Widget::field",
        "Struct: crate::TupleWidget",
        "Struct: crate::UnitWidget",
        "Function: crate::documented_function",
        "Function: crate::included_documentation",
        "Function: crate::typst_documentation",
        "Module: crate::nested",
        "Function: crate::nested::nested_function",
        " Field documentation.",
        " Function documentation.",
        " Nested function documentation.",
        "Command-documentation.",
        "# Generated section\n\nSection body",
        "#[doc = include_str(\"gwr-docpp/tests/fixtures/included.txt\")",
        "#[doc = typst(",
        "Const",
        "Enum",
        "ExternCrate",
        "Impl",
        "Static",
        "Trait",
        "Type",
        "Union",
        "Use",
    ] {
        assert!(dump.contains(expected), "missing {expected:?} in:\n{dump}");
    }
    // Syn 1 treats an Edition 2024 `unsafe extern` block as verbatim tokens,
    // while Syn 3 parses it as a foreign module. Both must be safely ignored.
    assert!(
        dump.lines()
            .any(|line| line == "ForeignMod" || line == "Verbatim"),
        "missing ignored foreign module in:\n{dump}"
    );
    assert!(!dump.contains("Field: crate::TupleWidget"));
    assert!(!dump.contains("Field: crate::UnitWidget"));
    assert!(!dump.contains("warn(unused)"));

    assert!(
        run.stderr()
            .contains("WARNING: module crate::missing not found"),
        "unexpected stderr:\n{}",
        run.stderr()
    );
}

#[test]
fn rejects_an_unknown_top_level_path() {
    let run = run_builder("crate::unknown", &[]);

    assert!(!run.output.status.success());
    assert!(
        run.stderr().contains("ERROR: crate::unknown not found"),
        "unexpected stderr:\n{}",
        run.stderr()
    );
}

#[test]
fn rejects_a_top_level_path_without_a_toc() {
    let run = run_builder("crate::nested", &[]);

    assert!(!run.output.status.success());
    assert!(
        run.stderr()
            .contains("ERROR: crate::nested does not contain TOC"),
        "unexpected stderr:\n{}",
        run.stderr()
    );
}
