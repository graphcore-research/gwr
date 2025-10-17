// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! By making the gwr-developer-guide a Rust package that is included in the
//! workspace it can be tested in a similar way to the Rust documentation. The
//! `mdbook_build` test aims to ensure that the mdBook build process remains
//! warning and error free, which should avoid the book containing Rust code
//! examples which do not compile or any broken links.
//!
//! Unlike normal Cargo tests the generated output will be written into the
//! source tree, with the rendered mdBook using the `book` directory, and the
//! temporary files used to check compilation of code examples using the
//! `doctest_cache` directory.
//!
//! As the workspace target directory cannot be used in this case (due to an
//! issue experienced with mdbook-keeper) the use of `cargo clean` will not
//! remove these build files as would be expected.
//!
//! When `mdbook build` is invoked directly from the command line the generated
//! output will also be written to the `book` (and `doctest_cache`) directory.

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::str;

    use strip_ansi_escapes::strip_str;

    #[test]
    fn mdbook_build() {
        let mdbook_output = Command::new("mdbook")
            .arg("build")
            .output()
            .expect("Failed to build gwr-developer-guide mdBook");

        let stdout =
            strip_str(str::from_utf8(&mdbook_output.stderr).expect("Failed to get command output"));

        assert!(
            !stdout.contains("(Panicked)"),
            "Error emitted during mdBook build:\n{stdout}"
        );
        assert!(
            !stdout.contains("[ERROR]"),
            "Error emitted during mdBook build:\n{stdout}"
        );
        assert!(
            !stdout.contains("error:"),
            "Error emitted during mdBook build:\n{stdout}"
        );
        assert!(
            !stdout.contains("[WARN]"),
            "Warning emitted during mdBook build:\n{stdout}"
        );
        assert!(
            !stdout.contains("warning"),
            "Warning emitted during mdBook build:\n{stdout}"
        );
    }
}
