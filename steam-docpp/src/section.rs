// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Process the `section!` documentation proc_macro.

use std::collections::HashMap;

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

        loop {
            if !parse_kv(&input, &mut items) {
                break;
            }
        }
        let title = items.get("title").expect("Missing required 'title'");
        let text = items.get("text").expect("Missing required 'text'");
        Ok(SectionDescriptor {
            title: title.clone(),
            text: text.clone(),
        })
    }
}

fn parse_kv(input: &ParseStream, items: &mut HashMap<String, String>) -> bool {
    let key = match input.parse::<Ident>() {
        Ok(key) => key.to_string(),
        Err(_) => return false,
    };
    if input.parse::<Eq>().is_err() {
        return false;
    }
    let value = match input.parse::<LitStr>() {
        Ok(value) => value.value().to_string(),
        Err(_) => return false,
    };

    items.insert(key, value);

    input.parse::<Comma>().is_ok()
}

pub fn process(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let section_descriptor = syn::parse_macro_input!(input as SectionDescriptor);
    handle_error(|| {
        let mut output = String::new();
        output.push_str(format!("# {}\n\n", section_descriptor.title).as_str());
        output.push_str(format!("{}\n\n", section_descriptor.text).as_str());

        Ok(LitStr::new(output.as_str(), Span::call_site()).into_token_stream())
    })
    .into()
}
