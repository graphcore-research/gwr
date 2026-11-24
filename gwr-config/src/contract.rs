// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Reject Clap/Serde features outside the application contract.

use std::iter;

use heck::ToKebabCase;
use quote::ToTokens;
use syn::ext::IdentExt;
use syn::{Attribute, Expr, Field, Fields, ItemStruct, Lit, Meta, PathArguments, Type};

use crate::multi_source_config::{flattened, inner_type, properties};

pub(crate) fn validate(item: &ItemStruct, flag: Option<&str>) -> syn::Result<()> {
    if !matches!(item.fields, Fields::Named(_))
        || !item.generics.params.is_empty()
        || item.generics.where_clause.is_some()
    {
        return Err(unsupported(item));
    }
    check(&item.attrs, "command", &[("about", true)])?;
    for namespace in ["arg", "clap", "serde"] {
        check(&item.attrs, namespace, &[])?;
    }
    check(&item.attrs, "group", &[("id", true), ("multiple", true)])?;
    let group = item
        .attrs
        .iter()
        .any(|attribute| attribute.path().is_ident("group"));
    if exclusive_group(&item.attrs)? && flag.is_some() {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "put conf_file_flag on the parent configuration, not an exclusive group",
        ));
    }
    for meta in properties(&item.attrs, &["derive"])? {
        if meta.path().segments.last().is_some_and(|segment| {
            ["Parser", "Args", "Serialize", "Deserialize", "Default"]
                .iter()
                .any(|name| segment.ident == name)
        }) {
            return Err(unsupported(meta));
        }
    }
    for field in &item.fields {
        validate_field(field, group)?;
    }
    // cfg_attr could otherwise smuggle unvalidated configuration metadata in.
    for attributes in iter::once(&item.attrs).chain(item.fields.iter().map(|field| &field.attrs)) {
        if let Some(attribute) = attributes
            .iter()
            .find(|attribute| attribute.path().is_ident("cfg_attr"))
        {
            return Err(unsupported(attribute));
        }
    }
    if flag.is_some_and(|field| {
        field.trim_start_matches("--").is_empty()
            || field.trim_start_matches("--").starts_with('-')
            || field.chars().any(char::is_whitespace)
    }) {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "conf_file_flag must be a nonempty long option name",
        ));
    }
    Ok(())
}

fn validate_field(field: &Field, group: bool) -> syn::Result<()> {
    if field
        .ident
        .as_ref()
        .unwrap()
        .to_string()
        .starts_with("__gwr_config_")
    {
        return Err(unsupported(field));
    }
    for namespace in ["arg", "clap"] {
        check(
            &field.attrs,
            namespace,
            &[
                ("short", false),
                ("short", true),
                ("long", false),
                ("long", true),
                ("default_value_t", true),
                ("value_parser", true),
                ("value_enum", false),
                ("required", true),
            ],
        )?;
    }
    check(&field.attrs, "command", &[("flatten", false)])?;
    check(
        &field.attrs,
        "serde",
        &[
            ("flatten", false),
            ("default", false),
            ("deserialize_with", true),
        ],
    )?;
    check(&field.attrs, "group", &[])?;
    required(field)?;
    let args = properties(&field.attrs, &["arg", "clap"])?;
    let has_default = args
        .iter()
        .any(|property| property.path().is_ident("default_value_t"));
    if args
        .iter()
        .any(|property| property.path().is_ident("required"))
        && (inner_type(&field.ty, "Vec").is_none() || has_default)
    {
        return Err(syn::Error::new_spanned(
            field,
            "gwr-config supports required only on Vec<T> fields without defaults",
        ));
    }
    if group && (inner_type(&field.ty, "Option").is_none() || has_default) {
        return Err(syn::Error::new_spanned(
            field,
            "gwr-config groups require Option<T> fields without defaults",
        ));
    }
    let serde_flatten = properties(&field.attrs, &["serde"])?
        .iter()
        .any(|property| property.path().is_ident("flatten"));
    if flattened(field) != serde_flatten {
        return Err(unsupported(field));
    }
    if flattened(field) {
        if group
            || !properties(&field.attrs, &["arg", "clap"])?.is_empty()
            || properties(&field.attrs, &["serde"])?.len() != 1
        {
            return Err(unsupported(field));
        }
        if !matches!(&field.ty, Type::Path(path) if path.qself.is_none() && path.path.segments.iter().all(|segment| matches!(segment.arguments, PathArguments::None)))
        {
            return Err(unsupported(&field.ty));
        }
    }
    Ok(())
}

pub(crate) fn long_name(field: &Field) -> syn::Result<Option<String>> {
    let mut name = None;
    for meta in properties(&field.attrs, &["arg", "clap"])? {
        match &meta {
            Meta::Path(path) if path.is_ident("long") => {
                name = Some(
                    field
                        .ident
                        .as_ref()
                        .unwrap()
                        .unraw()
                        .to_string()
                        .to_kebab_case(),
                );
            }
            Meta::NameValue(value) if value.path.is_ident("long") => {
                let Expr::Lit(lit) = &value.value else {
                    return Err(unsupported(meta));
                };
                let Lit::Str(value) = &lit.lit else {
                    return Err(unsupported(meta));
                };
                name = Some(value.value().trim_start_matches('-').to_owned());
            }
            _ => (),
        }
    }
    Ok(name)
}

pub(crate) fn exclusive_group(attributes: &[Attribute]) -> syn::Result<bool> {
    let mut exclusive = false;
    for meta in properties(attributes, &["group"])? {
        if let Meta::NameValue(value) = &meta {
            match (&value.path, &value.value) {
                (path, Expr::Lit(literal)) if path.is_ident("multiple") => {
                    if let Lit::Bool(boolean) = &literal.lit {
                        exclusive = !boolean.value;
                    } else {
                        return Err(unsupported(meta));
                    }
                }
                (path, Expr::Lit(literal))
                    if path.is_ident("id") && matches!(literal.lit, Lit::Str(_)) => {}
                _ => return Err(unsupported(meta)),
            }
        }
    }
    Ok(exclusive)
}

pub(crate) fn required(field: &Field) -> syn::Result<bool> {
    let mut required = false;
    for meta in properties(&field.attrs, &["arg", "clap"])? {
        if let Meta::NameValue(value) = &meta
            && value.path.is_ident("required")
        {
            if let Expr::Lit(literal) = &value.value
                && let Lit::Bool(boolean) = &literal.lit
            {
                required = boolean.value;
                continue;
            }
            return Err(unsupported(meta));
        }
    }
    Ok(required)
}

fn check(attributes: &[Attribute], namespace: &str, allowed: &[(&str, bool)]) -> syn::Result<()> {
    for meta in properties(attributes, &[namespace])? {
        if !allowed.iter().any(|(name, value)| {
            meta.path().is_ident(name)
                && matches!(
                    (&meta, value),
                    (Meta::Path(_), false) | (Meta::NameValue(_), true)
                )
        }) {
            return Err(unsupported(meta));
        }
    }
    Ok(())
}

fn unsupported(node: impl ToTokens) -> syn::Error {
    syn::Error::new_spanned(
        node,
        "unsupported gwr-config attribute or shape; see multi_source_config's supported subset",
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_config_flag_on_exclusive_group() {
        let item = syn::parse_str("#[group(id = \"input\", multiple = false)] struct Input { #[arg(long)] log: Option<String> }").unwrap();
        let expanded =
            crate::multi_source_config::multi_source_config_impl("", Some("conf-file"), item)
                .to_string();
        assert!(expanded.contains("compile_error"));
    }

    #[test]
    fn rejects_unsupported_contract() {
        for input in [
            "struct Config { #[arg(long, requires = \"other\")] value: Option<String> }",
            "struct Config { #[arg(skip)] value: Option<String> }",
            "struct Config { #[command(flatten)] value: Nested }",
            "struct Config<T> { value: T }",
            "struct Config { #[cfg_attr(unix, arg(skip))] value: String }",
            "struct Config { #[arg(long, required = true, default_value_t = 7)] value: Option<u64> }",
            "#[group(id = \"input\", multiple = false)] struct Config { #[arg(long, default_value_t = 7)] value: Option<u64> }",
        ] {
            let item = syn::parse_str(input).unwrap();
            let expanded =
                crate::multi_source_config::multi_source_config_impl("", None, item).to_string();
            assert!(
                expanded.contains("compile_error"),
                "accepted unsupported input: {input}"
            );
        }
    }
}
