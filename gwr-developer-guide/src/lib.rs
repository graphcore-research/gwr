// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

#![doc(test(attr(deny(unused_must_use))))]
#![doc = std::include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::str;

    use strip_ansi_escapes::strip_str;

    #[test]
    #[cfg_attr(
        coverage,
        ignore = "mdbook-linkcheck is environment-sensitive and is not useful for Rust coverage"
    )]
    fn mdbook_build() {
        let mdbook_output = Command::new("mdbook")
            .arg("build")
            .output()
            .expect("Failed to build gwr-developer-guide mdBook");

        let stderr =
            strip_str(str::from_utf8(&mdbook_output.stderr).expect("Failed to get command output"));

        // Catch all signal terminations and non-zero exits
        assert!(
            mdbook_output.status.success(),
            "mdbook build did not exit successfully:\n{stderr}"
        );

        // Attempt to catch warnings and errors emitted despite zero exit status
        assert!(
            !stderr.contains("(Panicked)"),
            "Error emitted during mdBook build:\n{stderr}"
        );
        assert!(
            !stderr.contains("[ERROR]"),
            "Error emitted during mdBook build:\n{stderr}"
        );
        assert!(
            !stderr.contains("error:"),
            "Error emitted during mdBook build:\n{stderr}"
        );
        assert!(
            !stderr.contains("[WARN]"),
            "Warning emitted during mdBook build:\n{stderr}",
        );
        assert!(
            !stderr.contains("warning"),
            "Warning emitted during mdBook build:\n{stderr}"
        );
    }
}
