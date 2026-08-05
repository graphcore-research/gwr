// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Process the `section!` documentation proc_macro.

use std::collections::HashMap;
use std::fmt::Write;

use proc_macro2::Span;
use quote::ToTokens;
use syn::parse::{self, Parse, ParseStream};
use syn::token::{Comma, Eq};
use syn::{Ident, LitStr};

use crate::helpers::handle_error;

/// Structure to store the command argument from within the macro! call.
#[derive(Debug)]
struct SectionDescriptor {
    title: String,
    text: String,
}

/// Implementation to parse the token stream and convert it to a
/// [`SectionDescriptor`]
impl Parse for SectionDescriptor {
    fn parse(input: ParseStream) -> parse::Result<Self> {
        let mut items = HashMap::new();

        while !input.is_empty() {
            parse_kv(input, &mut items)?;

            if input.is_empty() {
                break;
            }

            input.parse::<Comma>()?;
        }
        let title = items
            .remove("title")
            .ok_or_else(|| input.error("missing required `title` argument"))?;
        let text = items
            .remove("text")
            .ok_or_else(|| input.error("missing required `text` argument"))?;
        Ok(SectionDescriptor { title, text })
    }
}

fn parse_kv(input: ParseStream, items: &mut HashMap<String, String>) -> syn::Result<()> {
    let key: Ident = input.parse()?;
    input.parse::<Eq>()?;
    let value: LitStr = input.parse()?;

    items.insert(key.to_string(), value.value());

    Ok(())
}

pub fn process(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let section_descriptor = syn::parse_macro_input!(input as SectionDescriptor);
    handle_error(|| {
        let mut output = String::new();
        let _ = write!(output, "# {}\n\n", section_descriptor.title);
        let _ = write!(output, "{}\n\n", section_descriptor.text);

        Ok(LitStr::new(output.as_str(), Span::call_site()).into_token_stream())
    })
    .into()
}
