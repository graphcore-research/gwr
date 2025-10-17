// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Model builder

extern crate proc_macro;

use quote::quote;
use syn::{self, DeriveInput, parse_macro_input};

/// Create a std::fmt::Display implementation for a struct with an Entity.
#[proc_macro_derive(EntityDisplay)]
pub fn entity_display(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let DeriveInput {
        ident, generics, ..
    } = parse_macro_input!(input);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let output = quote! {
        impl #impl_generics std::fmt::Display for #ident #ty_generics #where_clause {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.entity.fmt(f)
            }
        }
    };

    output.into()
}

/// Create a default (empty) implementation of Runnable.
#[proc_macro_derive(Runnable)]
pub fn runnable(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let DeriveInput {
        ident, generics, ..
    } = parse_macro_input!(input);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let output = quote! {
        #[async_trait(?Send)]
        impl #impl_generics gwr_engine::traits::Runnable for #ident #ty_generics #where_clause {}
    };

    output.into()
}
