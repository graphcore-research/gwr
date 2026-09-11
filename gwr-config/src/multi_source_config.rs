// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

//! Sparse source values, merged before applying typed Rust defaults.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, ExprPath, Field, Fields, GenericArgument, Ident, ItemStruct, Lit, LitByteStr,
    LitStr, Meta, PathArguments, Token, Type, Visibility, parse_quote,
};

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
    if !properties(&sparse.attrs, &["group"])?
        .iter()
        .any(|meta| meta.path().is_ident("id"))
    {
        let group_id = name.unraw().to_string();
        sparse.attrs.push(parse_quote!(#[group(id = #group_id)]));
    }
    let mut deserializers = Vec::new();
    for field in &mut sparse.fields {
        if flattened(field) {
            if let Type::Path(path) = &mut field.ty {
                let segment = path.path.segments.last_mut().unwrap();
                segment.ident = format_ident!("{}Partial", segment.ident);
            }
        } else {
            if inner_type(&field.ty, "Option").is_none() {
                deserializers.push(configure_scalar_deserializer(&name, field)?);
                let field_type = &field.ty;
                field.ty = parse_quote!(Option<#field_type>);
            }
            configure_field(field)?;
        }
    }
    // Only defaultable leaves expose direct Clap parsing. Delegate to the
    // sparse representation so typed fallbacks never pass through CLI text.
    let defaultable = fields.iter().all(|field| {
        !flattened(field)
            && (inner_type(&field.ty, "Option").is_some() || fallback(field).is_some())
    });
    let factory = if defaultable {
        let parser = leaf_parser(&name, &fields, &sparse, has_conf_file_flag, exclusive);
        for field in &mut item.fields {
            configure_serde_default(&mut field.attrs)?;
        }
        quote! {
            #parser
            impl Default for #name {
                fn default() -> Self { Self::partial_to_config(#partial::default()) }
            }
        }
    } else {
        quote! {
            impl clap::CommandFactory for #name {
                fn command() -> clap::Command { <#partial as clap::CommandFactory>::command() }
                fn command_for_update() -> clap::Command { <#partial as clap::CommandFactory>::command_for_update() }
            }
        }
    };
    item.attrs.retain(|attribute| !clap_attr(attribute));
    item.attrs.insert(
        0,
        parse_quote!(#[derive(serde::Serialize, serde::Deserialize)]),
    );
    for field in &mut item.fields {
        field.attrs.retain(|attribute| !clap_attr(attribute));
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
        let field_ident = field.ident.as_ref().unwrap();
        let key = field_ident.unraw().to_string();
        let cfg_attributes: Vec<_> = field
            .attrs
            .iter()
            .filter(|attribute| attribute.path().is_ident("cfg"))
            .collect();
        let field_type = &field.ty;
        if flattened(field) {
            long_checks.push(
                quote! { #(#cfg_attributes)* if <#field_type>::__gwr_config_has_long_option(flag) { return true; } },
            );
            flatten_checks.push(quote! {
                #(#cfg_attributes)*
                const _: () = assert!(!<#field_type>::__GWR_CONFIG_HAS_CONF_FILE_FLAG,
                    "flattened configuration types must not declare conf_file_flag");
            });
            merge.push(quote! { #(#cfg_attributes)* { lower.#field_ident = <#field_type>::__gwr_config_merge_nested(lower.#field_ident, higher.#field_ident); } });
            strip.push(quote! { #(#cfg_attributes)* { config.#field_ident = <#field_type>::__gwr_config_strip_non_cli_values(config.#field_ident, matches); } });
            prune.push(quote! { #(#cfg_attributes)* <#field_type>::__gwr_config_prune_overridden_source_values(raw, &higher.#field_ident); });
            finish.push(quote! { #(#cfg_attributes)* #field_ident: <#field_type>::partial_to_config(config.#field_ident) });
        } else {
            if let Some(long) = contract::long_name(field)? {
                let bytes = LitByteStr::new(long.as_bytes(), field_ident.span());
                long_checks.push(
                    quote! { #(#cfg_attributes)* if matches!(flag.as_bytes(), #bytes) { return true; } },
                );
            }
            merge.push(quote! { #(#cfg_attributes)* { if higher.#field_ident.is_some() { lower.#field_ident = higher.#field_ident; } } });
            clear.push(quote! { #(#cfg_attributes)* { lower.#field_ident = None; } });
            count.push(quote! { #(#cfg_attributes)* { count += usize::from(config.#field_ident.is_some()); } });
            conflicts.push(quote! { #(#cfg_attributes)* {
                if config.#field_ident.is_some() {
                    let arg = command.get_arguments().find(|arg| arg.get_id().as_str() == #key).unwrap();
                    selected.push(arg.get_long().map_or_else(|| #key.to_owned(), |long| format!("--{long}")));
                }
            } });
            strip.push(quote! { #(#cfg_attributes)* {
                if matches.value_source(#key) != Some(clap::parser::ValueSource::CommandLine) { config.#field_ident = None; }
            } });
            prune.push(quote! { #(#cfg_attributes)* {
                if (#exclusive && Self::__gwr_config_present_count(higher) != 0) || higher.#field_ident.is_some() {
                    if let figment::value::Value::Dict(_, dict) = raw { dict.remove(#key); }
                }
            } });
            let value = match (inner_type(field_type, "Option").is_some(), fallback(field)) {
                (true, Some(expression)) => {
                    quote!(config.#field_ident.or_else(|| Some(#expression)))
                }
                (false, Some(expression)) => {
                    quote!(config.#field_ident.unwrap_or_else(|| #expression))
                }
                (true, None) => quote!(config.#field_ident),
                (false, None)
                    if properties(&field.attrs, &["serde"])?
                        .iter()
                        .any(|property| property.path().is_ident("default")) =>
                {
                    quote!(config.#field_ident.unwrap_or_default())
                }
                (false, None) => {
                    quote!(config.#field_ident.unwrap_or_else(|| panic!("missing required configuration value `{}`", #key)))
                }
            };
            finish.push(quote!(#(#cfg_attributes)* #field_ident: #value));
            if contract::required(field)? {
                let missing = if inner_type(field_type, "Vec").is_some() {
                    quote!(config.#field_ident.as_ref().is_none_or(|value| value.is_empty()))
                } else {
                    quote!(config.#field_ident.is_none())
                };
                required.push(quote! { #(#cfg_attributes)* if #missing { panic!("missing required configuration value `{}`", #key); } });
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
            #(#deserializers)*
        }
    })
}

fn leaf_parser(
    name: &Ident,
    fields: &Fields,
    sparse: &ItemStruct,
    has_conf_file_flag: bool,
    exclusive: bool,
) -> TokenStream {
    let partial = &sparse.ident;
    let mut members = Vec::new();
    let mut updates = Vec::new();
    let mut clear = Vec::new();
    for field in fields {
        let field_ident = field.ident.as_ref().unwrap();
        let cfg_attributes: Vec<_> = field
            .attrs
            .iter()
            .filter(|attribute| attribute.path().is_ident("cfg"))
            .collect();
        members.push(quote! { #(#cfg_attributes)* #field_ident: config.#field_ident });
        let value = if inner_type(&field.ty, "Option").is_some() {
            clear.push(quote! { #(#cfg_attributes)* { self.#field_ident = None; } });
            quote!(Some(value))
        } else {
            quote!(value)
        };
        updates.push(quote! { #(#cfg_attributes)* if let Some(value) = config.#field_ident { self.#field_ident = #value; } });
    }
    let (helper, parser, convert) = if has_conf_file_flag {
        // Capture the sparse shape before the layered-only file flag is added.
        // A const scope keeps this helper out of the generated public API.
        let mut helper = sparse.clone();
        helper.ident = format_ident!("__GwrConfigCliFor{}", name.unraw());
        helper.vis = Visibility::Inherited;
        helper
            .attrs
            .push(parse_quote!(#[allow(non_camel_case_types)]));
        let parser = helper.ident.clone();
        (
            quote!(#helper),
            parser,
            quote! {
                let config = #partial {
                    #(#members,)*
                    __gwr_config_conf_file: std::path::PathBuf::default(),
                };
            },
        )
    } else {
        (TokenStream::new(), partial.clone(), TokenStream::new())
    };
    quote! {
        const _: () = {
            #helper
            impl clap::Parser for #name {}
            impl clap::CommandFactory for #name {
                fn command() -> clap::Command {
                    <#parser as clap::CommandFactory>::command()
                }
                fn command_for_update() -> clap::Command {
                    <#parser as clap::CommandFactory>::command_for_update()
                }
            }
            impl clap::Args for #name {
                fn group_id() -> Option<clap::Id> { <#parser as clap::Args>::group_id() }
                fn augment_args(command: clap::Command) -> clap::Command {
                    <#parser as clap::Args>::augment_args(command)
                }
                fn augment_args_for_update(command: clap::Command) -> clap::Command {
                    <#parser as clap::Args>::augment_args_for_update(command)
                }
            }
            impl clap::FromArgMatches for #name {
                fn from_arg_matches(matches: &clap::ArgMatches) -> std::result::Result<Self, clap::Error> {
                    <Self as clap::FromArgMatches>::from_arg_matches_mut(&mut matches.clone())
                }
                fn from_arg_matches_mut(matches: &mut clap::ArgMatches) -> std::result::Result<Self, clap::Error> {
                    let config = <#parser as clap::FromArgMatches>::from_arg_matches_mut(matches)?;
                    #convert
                    std::result::Result::Ok(Self::partial_to_config(config))
                }
                fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> std::result::Result<(), clap::Error> {
                    <Self as clap::FromArgMatches>::update_from_arg_matches_mut(self, &mut matches.clone())
                }
                fn update_from_arg_matches_mut(&mut self, matches: &mut clap::ArgMatches) -> std::result::Result<(), clap::Error> {
                    let config = <#parser as clap::FromArgMatches>::from_arg_matches_mut(matches)?;
                    #convert
                    if #exclusive && Self::__gwr_config_present_count(&config) != 0 { #(#clear)* }
                    #(#updates)*
                    std::result::Result::Ok(())
                }
            }
        };
    }
}

fn configure_scalar_deserializer(name: &Ident, field: &mut Field) -> syn::Result<TokenStream> {
    let field_type = &field.ty;
    let field_ident = field.ident.as_ref().unwrap();
    let adapter = format_ident!("__gwr_config_deserialize_{}", field_ident.unraw());
    let cfg_attributes: Vec<_> = field
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("cfg"))
        .cloned()
        .collect();
    let mut deserializers = Vec::new();
    for attribute in &mut field.attrs {
        if !attribute.path().is_ident("serde") {
            continue;
        }
        let mut properties =
            attribute.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        for property in &mut properties {
            if let Meta::NameValue(value) = property
                && value.path.is_ident("deserialize_with")
            {
                let Expr::Lit(literal) = &value.value else {
                    return Err(syn::Error::new_spanned(
                        &value.value,
                        "expected a deserializer path string",
                    ));
                };
                let Lit::Str(path) = &literal.lit else {
                    return Err(syn::Error::new_spanned(
                        &value.value,
                        "expected a deserializer path string",
                    ));
                };
                let deserialize: ExprPath = path.parse()?;
                let adapter_path = LitStr::new(&format!("{name}::{adapter}"), path.span());
                value.value = parse_quote!(#adapter_path);
                deserializers.push(quote! {
                    #(#cfg_attributes)*
                    fn #adapter<'de, D: serde::Deserializer<'de>>(deserializer: D)
                        -> std::result::Result<std::option::Option<#field_type>, D::Error>
                    {
                        #deserialize(deserializer).map(std::option::Option::Some)
                    }
                });
            }
        }
        *attribute = parse_quote!(#[serde(#properties)]);
    }
    Ok(quote!(#(#deserializers)*))
}

fn configure_field(field: &mut Field) -> syn::Result<()> {
    let default = fallback(field);
    let value_enum = properties(&field.attrs, &["arg", "clap"])?
        .iter()
        .any(|property| property.path().is_ident("value_enum"));
    let optional = inner_type(&field.ty, "Option");
    let field_type = optional.unwrap_or(&field.ty);
    for attr in &mut field.attrs {
        if attr.path().is_ident("arg") || attr.path().is_ident("clap") {
            let argument_properties =
                attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
            let argument_properties: Vec<_> = argument_properties
                .into_iter()
                .filter(|property| {
                    !property.path().is_ident("default_value_t")
                        && !property.path().is_ident("required")
                })
                .collect();
            *attr = parse_quote!(#[arg(#(#argument_properties),*)]);
        }
    }
    configure_serde_default(&mut field.attrs)?;
    if is_type(field_type, "bool") {
        field.attrs.push(
            parse_quote!(#[arg(action = clap::ArgAction::Set, num_args = 0..=1,
            default_missing_value = "true", require_equals = true)]),
        );
    }
    // Describe typed fallbacks without invoking CLI value parsers.
    if let Some(expression) = default {
        let description: Expr = if value_enum {
            parse_quote!({ let value: #field_type = #expression;
                clap::ValueEnum::to_possible_value(&value).map_or_else(
                    || String::from("[default: non-CLI value]"),
                    |possible| format!("[default: {}]", possible.get_name())) })
        } else {
            parse_quote!({ let value: #field_type = #expression; format!("[default: {:?}]", value) })
        };
        let docs: Vec<_> = field
            .attrs
            .iter()
            .filter_map(|attribute| {
                if !attribute.path().is_ident("doc") {
                    return None;
                }
                let Meta::NameValue(value) = &attribute.meta else {
                    return None;
                };
                Some(value.value.clone())
            })
            .collect();
        field.attrs.push(parse_quote!(#[arg(
            help = {
                let lines: &[&str] = &[#(#docs),*];
                let mut text = lines.iter().map(|line| line.trim()).collect::<Vec<_>>().join(" ");
                if !text.is_empty() { text.push(' '); }
                text.push_str(&#description);
                text
            },
            long_help = {
                let default_description = || #description;
                let docs: &[&str] = &[#(#docs),*];
                let lines: Vec<_> = docs.iter()
                    .flat_map(|doc| doc.split('\n'))
                    .map(str::trim)
                    .collect();
                let paragraphs: Vec<_> = lines.split(|line| line.is_empty())
                    .filter(|paragraph| !paragraph.is_empty())
                    .map(|paragraph| paragraph.join(" "))
                    .collect();
                if paragraphs.len() > 1 {
                    let text = format!("{} {}", paragraphs.join("\n\n"), default_description());
                    clap::builder::Resettable::Value(clap::builder::StyledStr::from(text))
                } else {
                    clap::builder::Resettable::Reset
                }
            }
        )]));
    }
    Ok(())
}

fn configure_serde_default(attributes: &mut Vec<Attribute>) -> syn::Result<()> {
    if !properties(attributes, &["serde"])?
        .iter()
        .any(|property| property.path().is_ident("default"))
    {
        attributes.push(parse_quote!(#[serde(default)]));
    }
    Ok(())
}

fn fallback(field: &Field) -> Option<Expr> {
    properties(&field.attrs, &["arg", "clap"])
        .ok()?
        .into_iter()
        .find_map(|property| {
            if let Meta::NameValue(value) = property
                && value.path.is_ident("default_value_t")
            {
                return Some(value.value);
            }
            None
        })
}

pub(crate) fn flattened(field: &Field) -> bool {
    properties(&field.attrs, &["command"]).is_ok_and(|path| {
        path.iter()
            .any(|property| property.path().is_ident("flatten"))
    })
}

fn clap_attr(attr: &Attribute) -> bool {
    ["arg", "clap", "command", "group"]
        .iter()
        .any(|name| attr.path().is_ident(name))
}

pub(crate) fn inner_type<'field>(field_type: &'field Type, name: &str) -> Option<&'field Type> {
    if !is_type(field_type, name) {
        return None;
    }
    let Type::Path(path) = field_type else {
        return None;
    };
    let PathArguments::AngleBracketed(args) = &path.path.segments.last()?.arguments else {
        return None;
    };
    match args.args.first()? {
        GenericArgument::Type(field_type) => Some(field_type),
        _ => None,
    }
}

fn is_type(field_type: &Type, name: &str) -> bool {
    matches!(field_type, Type::Path(path) if path.qself.is_none() && path.path.segments.last().is_some_and(|segment| segment.ident == name))
}

pub(crate) fn properties(attributes: &[Attribute], names: &[&str]) -> syn::Result<Vec<Meta>> {
    let mut result = Vec::new();
    for attr in attributes
        .iter()
        .filter(|attribute| names.iter().any(|name| attribute.path().is_ident(name)))
    {
        result.extend(attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?);
    }
    Ok(result)
}
