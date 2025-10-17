// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Shared helper functions

use std::env;

use proc_macro2::Span;
use quote::ToTokens;
use syn::parse::{self, Parse, ParseStream};
use syn::{LitStr, Result};

/// Structure to store the command argument from within the macro! call.
#[derive(Debug)]
pub struct CommandDescriptor {
    pub cmd: String,
}

/// Implementation to parse the token stream and convert it to a
/// [`CommandDescriptor`]
impl Parse for CommandDescriptor {
    fn parse(input: ParseStream) -> parse::Result<Self> {
        let command = input.parse::<LitStr>()?;
        Ok(CommandDescriptor {
            cmd: command.value(),
        })
    }
}

#[must_use]
pub fn env_doc_builder() -> bool {
    env::var("GWR_DOC_BUILDER").is_ok()
}

#[must_use]
pub fn unprocessed(input: &proc_macro2::TokenStream, prefix: &str) -> proc_macro2::TokenStream {
    handle_error(|| {
        Ok(LitStr::new(
            format!("#[doc = {prefix}({input})").as_str(),
            Span::call_site(),
        )
        .into_token_stream())
    })
}

pub fn handle_error(
    cb: impl FnOnce() -> Result<proc_macro2::TokenStream>,
) -> proc_macro2::TokenStream {
    match cb() {
        Ok(tokens) => tokens,
        Err(e) => e.to_compile_error(),
    }
}
