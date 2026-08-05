// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Process the `include_str!` documentation proc_macro.

use std::fs::File;
use std::io::Read;

use gwr_doc_builder::helpers::{CommandDescriptor, env_doc_builder, handle_error, unprocessed};
use proc_macro2::Span;
use quote::ToTokens;
use syn::{Error, LitStr};

pub fn process(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    if env_doc_builder() {
        return unprocessed(&input.into(), "include_str").into();
    }

    let command_descriptor = syn::parse_macro_input!(input as CommandDescriptor);
    handle_error(|| {
        let file_name = &command_descriptor.cmd;
        let span = command_descriptor.span;

        let mut file = File::open(file_name)
            .map_err(|_| Error::new(span, format!("failed to open `{file_name}`")))?;
        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|_| Error::new(span, format!("failed to read `{file_name}` contents")))?;

        Ok(LitStr::new(content.as_str(), Span::call_site()).into_token_stream())
    })
    .into()
}
