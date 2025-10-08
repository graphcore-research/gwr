// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Process the `typst!` documentation proc_macro.

use std::io::Write;
use std::process::Command;

use proc_macro2::Span;
use quote::ToTokens;
use syn::{Error, LitStr};
use tempfile::NamedTempFile;
use tramway_doc_builder::helpers::{CommandDescriptor, env_doc_builder, handle_error, unprocessed};

pub fn process(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    if env_doc_builder() {
        return unprocessed(&input.into(), "typst").into();
    }

    let command_descriptor = syn::parse_macro_input!(input as CommandDescriptor);
    handle_error(|| {
        let mut tmp = NamedTempFile::new().expect("Failed to create tmp file");
        // Creae a header that renders an SVG that is just the size of the content
        tmp.write_all(
            b"#set page(
                  height: auto,
                  width: auto,
                  margin: (x: 0pt, y: 0pt),
            )\n",
        )
        .expect("Failed to write to tmp file");
        tmp.write_all(command_descriptor.cmd.as_bytes())
            .expect("Failed to write to tmp file");

        let output = Command::new("typst")
            .arg("compile")
            .arg("-f") // Set the output format to SVG
            .arg("svg")
            .arg(tmp.path())
            .arg("/dev/stdout") // And get it written to stdout
            .output()
            .expect("\n\n**Failed to run typst - make sure it is on the path**");

        tmp.close().expect("Failed to close tmp file");

        if output.status.success() {
            let output = String::from_utf8_lossy(&output.stdout).to_string();

            Ok(LitStr::new(&output, Span::call_site()).into_token_stream())
        } else {
            let output = String::from_utf8_lossy(&output.stderr).to_string();
            Err(Error::new(Span::call_site(), output.as_str()))
        }
    })
    .into()
}
