// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_doc_builder::asciidoctor_pp::AsciiDoctorPreProcessor;

#[test]
fn preprocesses_documentation_text() {
    let preprocessor = AsciiDoctorPreProcessor::default();
    let cases = [
        (
            "strips at most one leading space",
            " one space\n  two spaces\nno space",
            2,
            "one space\n two spaces\nno space",
        ),
        (
            "reindents headings but not equals-prefixed text",
            "= Heading\n== Subheading\n=not a heading",
            4,
            "=== Heading\n==== Subheading\n=not a heading",
        ),
        (
            "converts every code link but leaves ordinary markdown links",
            "[`One`](crate::One) and [`Two`](crate::Two), not [three](crate::Three)",
            2,
            "<<crate::One,One>> and <<crate::Two,Two>>, not [three](crate::Three)",
        ),
    ];

    for (name, input, depth, expected) in cases {
        assert_eq!(
            preprocessor.preprocess_doc(input, depth),
            expected,
            "{name}"
        );
    }
}
