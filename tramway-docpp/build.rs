// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

#[cfg(feature = "typst")]
use std::process::Command;

fn main() {
    #[cfg(feature = "typst")]
    Command::new("cargo")
        .arg("install")
        .arg("typst-cli")
        .output()
        .expect("Failed to install typst");
}
