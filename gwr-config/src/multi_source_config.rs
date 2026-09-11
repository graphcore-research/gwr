// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

//! Sparse source values, merged before applying typed Rust defaults.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, Field, Fields, ItemStruct, Meta, Token, Type, parse_quote};

use crate::contract;

pub(crate) fn multi_source_config_impl(
    default_conf_file: &str,
    conf_file_flag: Option<&str>,
    item: ItemStruct,
) -> TokenStream {
    expand(default_conf_file, conf_file_flag, item).unwrap_or_else(syn::Error::into_compile_error)
}

// Keep the generated API together: most of this function is quoted Rust.
#[allow(clippy::too_many_lines)]
fn expand(
    default_conf_file: &str,
    conf_file_flag: Option<&str>,
    mut item: ItemStruct,
) -> syn::Result<TokenStream> {
    contract::validate(&item, conf_file_flag)?;
    let name = item.ident.clone();
    let partial = format_ident!("{}Partial", name);
    let fields = item.fields.clone();
    let exclusive = contract::exclusive_group(&item.attrs)?;
    let has_conf_file_flag = conf_file_flag.is_some();
    let mut sparse = item.clone();
    sparse.ident = partial.clone();
    sparse.attrs.insert(
        0,
        parse_quote!(#[derive(clap::Parser, serde::Serialize, serde::Deserialize, Default)]),
    );
    if !sparse.attrs.iter().any(|a| a.path().is_ident("group")) {
        let id = name.unraw().to_string();
        sparse.attrs.push(parse_quote!(#[group(id = #id)]));
    }
    for field in &mut sparse.fields {
        if flattened(field) {
            if let Type::Path(path) = &mut field.ty {
                let segment = path.path.segments.last_mut().unwrap();
                segment.ident = format_ident!("{}Partial", segment.ident);
            }
        } else {
            let ty = &field.ty;
            if inner_type(ty, "Option").is_none() {
                field.ty = parse_quote!(Option<#ty>);
            }
            configure_field(field)?;
        }
    }
    if let Some(flag) = conf_file_flag {
        let flag = flag.trim_start_matches("--");
        let Fields::Named(named) = &mut sparse.fields else {
            unreachable!()
        };
        named.named.push(parse_quote! {
            /// Path to a TOML configuration file; an empty path disables the file.
            #[serde(skip)]
            #[arg(long = #flag, value_name = "CONF_FILE", default_value_os_t = <#name>::static_conf_file_path(),
                value_parser = {
                    use clap::builder::TypedValueParser as _;
                    clap::builder::OsStringValueParser::new().map(std::path::PathBuf::from)
                })]
            __gwr_config_conf_file: std::path::PathBuf
        });
    }
    // Retain Args/Default for reusable leaf configurations. Layered parsing
    // always uses the sparse representation, never this direct parser.
    let defaultable = fields
        .iter()
        .all(|f| !flattened(f) && (inner_type(&f.ty, "Option").is_some() || fallback(f).is_some()));
    let factory = if defaultable {
        item.attrs.insert(
            0,
            parse_quote!(#[derive(clap::Parser, serde::Serialize, serde::Deserialize)]),
        );
        for field in &mut item.fields {
            configure_field(field)?;
        }
        quote! { impl Default for #name { fn default() -> Self { Self::partial_to_config(#partial::default()) } } }
    } else {
        item.attrs.retain(|a| !config_attr(a));
        for field in &mut item.fields {
            field.attrs.retain(|a| !config_attr(a));
        }
        quote! {
            impl clap::CommandFactory for #name {
                fn command() -> clap::Command { <#partial as clap::CommandFactory>::command() }
                fn command_for_update() -> clap::Command { <#partial as clap::CommandFactory>::command_for_update() }
            }
        }
    };
    let mut merge = Vec::new();
    let mut clear = Vec::new();
    let mut count = Vec::new();
    let mut strip = Vec::new();
    let mut prune = Vec::new();
    let mut finish = Vec::new();
    let mut required = Vec::new();
    let mut conflicts = Vec::new();
    let mut flatten_checks = Vec::new();
    let mut long_checks = Vec::new();
    for field in &fields {
        let id = field.ident.as_ref().unwrap();
        let key = id.unraw().to_string();
        let cfg: Vec<_> = field
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("cfg"))
            .collect();
        let ty = &field.ty;
        if flattened(field) {
            long_checks.push(
                quote! { #(#cfg)* if <#ty>::__gwr_config_has_long_option(flag) { return true; } },
            );
            flatten_checks.push(quote! {
                #(#cfg)*
                const _: () = assert!(!<#ty>::__GWR_CONFIG_HAS_CONF_FILE_FLAG,
                    "flattened configuration types must not declare conf_file_flag");
            });
            merge.push(quote! { #(#cfg)* { lower.#id = <#ty>::__gwr_config_merge_nested(lower.#id, higher.#id); } });
            strip.push(quote! { #(#cfg)* { config.#id = <#ty>::__gwr_config_strip_non_cli_values(config.#id, matches); } });
            prune.push(quote! { #(#cfg)* <#ty>::__gwr_config_prune_overridden_source_values(raw, &higher.#id); });
            finish.push(quote! { #(#cfg)* #id: <#ty>::partial_to_config(config.#id) });
        } else {
            if let Some(long) = contract::long_name(field)? {
                let bytes = syn::LitByteStr::new(long.as_bytes(), id.span());
                long_checks.push(
                    quote! { #(#cfg)* if matches!(flag.as_bytes(), #bytes) { return true; } },
                );
            }
            merge.push(quote! { #(#cfg)* { if higher.#id.is_some() { lower.#id = higher.#id; } } });
            clear.push(quote! { #(#cfg)* { lower.#id = None; } });
            count.push(quote! { #(#cfg)* { count += usize::from(config.#id.is_some()); } });
            conflicts.push(quote! { #(#cfg)* {
                if config.#id.is_some() {
                    let arg = command.get_arguments().find(|arg| arg.get_id().as_str() == #key).unwrap();
                    selected.push(arg.get_long().map_or_else(|| #key.to_owned(), |long| format!("--{long}")));
                }
            } });
            strip.push(quote! { #(#cfg)* {
                if matches.value_source(#key) != Some(clap::parser::ValueSource::CommandLine) { config.#id = None; }
            } });
            prune.push(quote! { #(#cfg)* {
                if (#exclusive && Self::__gwr_config_present_count(higher) != 0) || higher.#id.is_some() {
                    if let figment::value::Value::Dict(_, dict) = raw { dict.remove(#key); }
                }
            } });
            let value = match (inner_type(ty, "Option").is_some(), fallback(field)) {
                (true, Some(expr)) => quote!(config.#id.or_else(|| Some(#expr))),
                (false, Some(expr)) => quote!(config.#id.unwrap_or_else(|| #expr)),
                (true, None) => quote!(config.#id),
                (false, None) => {
                    quote!(config.#id.unwrap_or_else(|| panic!("missing required configuration value `{}`", #key)))
                }
            };
            finish.push(quote!(#(#cfg)* #id: #value));
            if contract::required(field)? {
                let missing = if inner_type(ty, "Vec").is_some() {
                    quote!(config.#id.as_ref().is_none_or(|value| value.is_empty()))
                } else {
                    quote!(config.#id.is_none())
                };
                required.push(quote! { #(#cfg)* if #missing { panic!("missing required configuration value `{}`", #key); } });
            }
        }
    }
    let path = if default_conf_file.is_empty() {
        quote! { path.set_extension("toml"); }
    } else {
        quote! { path.set_file_name(#default_conf_file); }
    };
    let selected_path = if conf_file_flag.is_some() {
        quote! {
            let explicit = matches.value_source("__gwr_config_conf_file") == Some(clap::parser::ValueSource::CommandLine);
            let path = if explicit { matches.get_one::<std::path::PathBuf>("__gwr_config_conf_file").unwrap().clone() }
                else { Self::static_conf_file_path() };
        }
    } else {
        quote! { let explicit = false; let path = Self::static_conf_file_path(); }
    };
    let flag_check = conf_file_flag.map(|flag| {
        let flag = flag.trim_start_matches("--");
        let message = format!(
            "conf_file_flag `--{flag}` conflicts with an existing long option or reserved --help"
        );
        quote! { const _: () = assert!(!<#name>::__gwr_config_has_long_option(#flag), #message); }
    });
    Ok(quote! {
        #item
        #sparse
        #factory
        #(#flatten_checks)*
        #flag_check
        #[allow(dead_code)]
        impl #name {
            #[doc(hidden)]
            pub const __GWR_CONFIG_HAS_CONF_FILE_FLAG: bool = #has_conf_file_flag;

            // Byte-string patterns allow const checks without duplicating Clap's
            // command construction or rejecting cfg-disabled fields.
            #[doc(hidden)]
            pub const fn __gwr_config_has_long_option(flag: &str) -> bool {
                #(#long_checks)*
                matches!(flag.as_bytes(), b"help")
            }

            fn parse_all_sources() -> Self {
                let matches = <#partial as clap::CommandFactory>::command().get_matches();
                Self::parse_all_sources_with_matches(&matches)
            }
            fn parse_all_sources_with_matches(matches: &clap::ArgMatches) -> Self {
                let cli = <#partial as clap::FromArgMatches>::from_arg_matches(matches).unwrap();
                let cli = Self::__gwr_config_strip_non_cli_values(cli, matches);
                #selected_path
                let lower = Self::__gwr_config_read_sources(&path, explicit, &cli);
                Self::partial_to_config(Self::clap_merge(lower, cli))
            }
            fn static_conf_file_path() -> std::path::PathBuf {
                let mut path = std::path::PathBuf::from(file!());
                #path
                path
            }
            fn figment_extract(source: figment::Figment) -> #partial { source.extract().unwrap() }
            fn figment_to_config(path: &std::path::Path, explicit: bool) -> #partial {
                Self::__gwr_config_read_sources(path, explicit, &#partial::default())
            }
            fn __gwr_config_read_sources(path: &std::path::Path, explicit: bool, cli: &#partial) -> #partial {
                use figment::providers::Format as _;
                if explicit && !path.as_os_str().is_empty() {
                    assert!(!path.is_dir(), "{} is not a file path", path.display());
                    assert!(path.exists(), "{} not found", path.display());
                }
                let env = figment::Figment::from(figment::providers::Env::prefixed("GWR_"));
                let env = Self::__gwr_config_extract_below(env, cli);
                let file = if path.as_os_str().is_empty() { #partial::default() } else {
                    let source = figment::Figment::from(figment::providers::Toml::file(path));
                    let mut raw: figment::value::Value = source.extract().unwrap();
                    // Discard overwritten fields before typed deserialization.
                    Self::__gwr_config_prune_overridden_source_values(&mut raw, cli);
                    Self::__gwr_config_prune_overridden_source_values(&mut raw, &env);
                    Self::__gwr_config_deserialize(&source, raw)
                };
                Self::__gwr_config_merge_nested(file, env)
            }
            fn __gwr_config_extract_below(source: figment::Figment, higher: &#partial) -> #partial {
                let mut raw = source.extract().unwrap();
                Self::__gwr_config_prune_overridden_source_values(&mut raw, higher);
                Self::__gwr_config_deserialize(&source, raw)
            }
            fn __gwr_config_deserialize(source: &figment::Figment, raw: figment::value::Value) -> #partial {
                raw.deserialize().map_err(|mut error| {
                    error.metadata = source.find_metadata(&error.path.join(".")).cloned();
                    error.profile = Some(source.profile().clone());
                    error
                }).unwrap()
            }
            #[doc(hidden)]
            pub fn __gwr_config_prune_overridden_source_values(raw: &mut figment::value::Value, higher: &#partial) { #(#prune)* }
            fn __gwr_config_present_count(config: &#partial) -> usize {
                let mut count = 0;
                #(#count)*
                count
            }
            #[doc(hidden)]
            pub fn __gwr_config_merge_nested(mut lower: #partial, higher: #partial) -> #partial {
                // A higher-priority selection replaces the whole exclusive group.
                if #exclusive && Self::__gwr_config_present_count(&higher) != 0 { #(#clear)* }
                #(#merge)*
                lower
            }
            #[doc(hidden)]
            pub fn __gwr_config_strip_non_cli_values(mut config: #partial, matches: &clap::ArgMatches) -> #partial {
                #(#strip)*
                config
            }
            fn clap_merge(lower: #partial, higher: #partial) -> #partial { Self::__gwr_config_merge_nested(lower, higher) }
            #[doc(hidden)]
            pub fn partial_to_config(config: #partial) -> Self {
                if #exclusive && Self::__gwr_config_present_count(&config) > 1 {
                    let command = <#partial as clap::CommandFactory>::command();
                    let mut selected: Vec<String> = Vec::new();
                    #(#conflicts)*
                    panic!("configuration values {} cannot be used with one another", selected.join(", "));
                }
                #(#required)*
                Self { #(#finish,)* }
            }
        }
    })
}

fn configure_field(field: &mut Field) -> syn::Result<()> {
    let default = fallback(field);
    let value_enum = properties(&field.attrs, &["arg", "clap"])?
        .iter()
        .any(|m| m.path().is_ident("value_enum"));
    for attr in &mut field.attrs {
        if attr.path().is_ident("arg") || attr.path().is_ident("clap") {
            let props = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
            let props: Vec<_> = props
                .into_iter()
                .filter(|m| !m.path().is_ident("default_value_t") && !m.path().is_ident("required"))
                .collect();
            *attr = parse_quote!(#[arg(#(#props),*)]);
        }
    }
    if !properties(&field.attrs, &["serde"])?
        .iter()
        .any(|m| m.path().is_ident("default"))
    {
        field.attrs.push(parse_quote!(#[serde(default)]));
    }
    let ty = inner_type(&field.ty, "Option").unwrap_or(&field.ty);
    if is_type(ty, "bool") {
        field.attrs.push(
            parse_quote!(#[arg(action = clap::ArgAction::Set, num_args = 0..=1,
            default_missing_value = "true", require_equals = true)]),
        );
    }
    // Describe typed fallbacks without invoking CLI value parsers.
    if let Some(expr) = default {
        let description: Expr = if value_enum {
            parse_quote!({ let value: #ty = #expr;
                clap::ValueEnum::to_possible_value(&value).map_or_else(
                    || String::from("[default: non-CLI value]"),
                    |possible| format!("[default: {}]", possible.get_name())) })
        } else {
            parse_quote!({ let value: #ty = #expr; format!("[default: {:?}]", value) })
        };
        let docs: Vec<_> = field
            .attrs
            .iter()
            .filter_map(|a| {
                if !a.path().is_ident("doc") {
                    return None;
                }
                let Meta::NameValue(v) = &a.meta else {
                    return None;
                };
                Some(v.value.clone())
            })
            .collect();
        field.attrs.push(parse_quote!(#[arg(help = {
            let lines: &[&str] = &[#(#docs),*];
            let mut text = lines.iter().map(|line| line.trim()).collect::<Vec<_>>().join(" ");
            if !text.is_empty() { text.push(' '); }
            text.push_str(&#description);
            text
        })]));
    }
    Ok(())
}

pub(crate) fn properties(attrs: &[Attribute], names: &[&str]) -> syn::Result<Vec<Meta>> {
    let mut result = Vec::new();
    for attr in attrs
        .iter()
        .filter(|a| names.iter().any(|n| a.path().is_ident(n)))
    {
        result.extend(attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?);
    }
    Ok(result)
}

fn fallback(field: &Field) -> Option<Expr> {
    properties(&field.attrs, &["arg", "clap"])
        .ok()?
        .into_iter()
        .find_map(|m| {
            if let Meta::NameValue(v) = m
                && v.path.is_ident("default_value_t")
            {
                return Some(v.value);
            }
            None
        })
}

pub(crate) fn flattened(field: &Field) -> bool {
    properties(&field.attrs, &["command"])
        .is_ok_and(|p| p.iter().any(|m| m.path().is_ident("flatten")))
}

fn config_attr(attr: &Attribute) -> bool {
    ["arg", "clap", "command", "serde", "group"]
        .iter()
        .any(|n| attr.path().is_ident(n))
}

fn is_type(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Path(p) if p.qself.is_none() && p.path.segments.last().is_some_and(|s| s.ident == name))
}

pub(crate) fn inner_type<'a>(ty: &'a Type, name: &str) -> Option<&'a Type> {
    if !is_type(ty, name) {
        return None;
    }
    let Type::Path(p) = ty else {
        return None;
    };
    let syn::PathArguments::AngleBracketed(args) = &p.path.segments.last()?.arguments else {
        return None;
    };
    match args.args.first()? {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    }
}
