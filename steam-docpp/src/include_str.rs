// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Process the `include_str!` documentation proc_macro.

use std::fs::File;
use std::io::Read;

use proc_macro2::Span;
use quote::ToTokens;
use steam_doc_builder::helpers::{CommandDescriptor, env_doc_builder, handle_error, unprocessed};
use syn::LitStr;

pub fn process(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    if env_doc_builder() {
        return unprocessed(input.into(), "include_str").into();
    }

    let command_descriptor = syn::parse_macro_input!(input as CommandDescriptor);
    let file_name = command_descriptor.cmd.clone();
    let mut file = File::open(command_descriptor.cmd)
        .unwrap_or_else(|_| panic!("Failed to open {}", file_name));
    let mut content = String::new();
    file.read_to_string(&mut content)
        .unwrap_or_else(|_| panic!("Failed to read {} contents", file_name));

    handle_error(|| Ok(LitStr::new(content.as_str(), Span::call_site()).into_token_stream())).into()
}
