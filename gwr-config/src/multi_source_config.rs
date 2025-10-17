// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

//! The implementation of the [macro@crate::multi_source_config] macro.
//!
//! Within this file [proc_macro2] types are used, with [syn] providing the
//! required parsing functionality and [mod@quote] allowing new AST to be
//! created as if writing Rust as normal.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use regex::Regex;
use syn::{Attribute, Expr, Fields, Ident, ItemStruct, Lit, Meta, Path, parse_quote, parse_str};

const CLAP: &str = "clap";
const CLAP_PARSER: &str = "Parser";
const FIGMENT: &str = "figment";
const FIGMENT_PROVIDERS: &str = "providers";
const FIGMENT_PROVIDERS_FORMAT: &str = "Format";
const SERDE: &str = "serde";
const SERDE_DESERIALIZE: &str = "Deserialize";
const SERDE_SERIALIZE: &str = "Serialize";

pub(crate) fn multi_source_config_impl(alt_conf_file: &str, mut item: ItemStruct) -> TokenStream {
    let mut result = Vec::new();

    let struct_type = &item.ident;

    let renamed_clap_parser = format_ident!("{}_{}_{}", struct_type, CLAP, CLAP_PARSER);
    let renamed_figment_providers_format = format_ident!(
        "{}_{}_{}_{}",
        struct_type,
        FIGMENT,
        FIGMENT_PROVIDERS,
        FIGMENT_PROVIDERS_FORMAT
    );
    let renamed_serde_deserialize =
        format_ident!("{}_{}_{}", struct_type, SERDE, SERDE_DESERIALIZE);
    let renamed_serde_serialize = format_ident!("{}_{}_{}", struct_type, SERDE, SERDE_SERIALIZE);

    result.push(generate_use_statements(
        &renamed_clap_parser,
        &renamed_figment_providers_format,
        &renamed_serde_deserialize,
        &renamed_serde_serialize,
    ));

    update_struct_attrs(
        &mut item.attrs,
        &renamed_clap_parser,
        &renamed_serde_deserialize,
        &renamed_serde_serialize,
    );
    update_field_attrs(struct_type, &mut item.fields);
    result.push(quote! {#item});

    result.push(generate_impl_block(
        struct_type,
        &item.fields,
        alt_conf_file,
    ));

    quote! {
        #(#result)*
    }
}

fn generate_use_statements(
    renamed_clap_parser: &Ident,
    renamed_figment_providers_format: &Ident,
    renamed_serde_deserialize: &Ident,
    renamed_serde_serialize: &Ident,
) -> TokenStream {
    let clap_parser = parse_str::<Path>(&format!("{CLAP}::{CLAP_PARSER}")).unwrap();
    let figment_providers_format = parse_str::<Path>(&format!(
        "{FIGMENT}::{FIGMENT_PROVIDERS}::{FIGMENT_PROVIDERS_FORMAT}"
    ))
    .unwrap();
    let serde_deserialize = parse_str::<Path>(&format!("{SERDE}::{SERDE_DESERIALIZE}")).unwrap();
    let serde_serialize = parse_str::<Path>(&format!("{SERDE}::{SERDE_SERIALIZE}")).unwrap();

    quote! {
        use #clap_parser as #renamed_clap_parser;
        use #figment_providers_format as #renamed_figment_providers_format;
        use #serde_deserialize as #renamed_serde_deserialize;
        use #serde_serialize as #renamed_serde_serialize;
    }
}

fn update_struct_attrs(
    attrs: &mut Vec<Attribute>,
    renamed_clap_parser: &Ident,
    renamed_serde_deserialize: &Ident,
    renamed_serde_serialize: &Ident,
) {
    check_derive_attrs(attrs);
    let new_derives = generate_derive_attrs(
        renamed_clap_parser,
        renamed_serde_deserialize,
        renamed_serde_serialize,
    );

    // Ensure that generated attributes are prepended to avoid hitting the
    // legacy_derive_helpers lint.
    // See https://github.com/rust-lang/rust/issues/79202 for further details.
    //
    // The range given to splice is to avoid any of the existing attributes
    // being replaced.
    attrs.splice(0..0, new_derives);
}

fn check_derive_attrs(attrs: &[Attribute]) {
    let clap_parser = Ident::new(CLAP_PARSER, Span::call_site());
    let clap_parser_full = parse_str::<Path>(&format!("{CLAP}::{CLAP_PARSER}")).unwrap();
    let serde_deserialize = Ident::new(SERDE_DESERIALIZE, Span::call_site());
    let serde_deserialize_full =
        parse_str::<Path>(&format!("{SERDE}::{SERDE_DESERIALIZE}")).unwrap();
    let serde_serialize = Ident::new(SERDE_SERIALIZE, Span::call_site());
    let serde_serialize_full = parse_str::<Path>(&format!("{SERDE}::{SERDE_SERIALIZE}")).unwrap();

    let error_msg = "This struct is annotated with #[multi_source_config] so cannot derive";

    for attr in attrs {
        if attr.path().is_ident("derive") {
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident(&clap_parser) {
                    panic!("{error_msg} {CLAP_PARSER}");
                } else if meta.path == clap_parser_full {
                    panic!("{error_msg} {CLAP}::{CLAP_PARSER}");
                } else if meta.path.is_ident(&serde_deserialize) {
                    panic!("{error_msg} {SERDE_DESERIALIZE}");
                } else if meta.path == serde_deserialize_full {
                    panic!("{error_msg} {SERDE}::{SERDE_DESERIALIZE}");
                } else if meta.path.is_ident(&serde_serialize) {
                    panic!("{error_msg} {SERDE_SERIALIZE}");
                } else if meta.path == serde_serialize_full {
                    panic!("{error_msg} {SERDE}::{SERDE_SERIALIZE}");
                }
                Ok(())
            });
        }
    }
}

fn generate_derive_attrs(
    renamed_clap_parser: &Ident,
    renamed_serde_deserialize: &Ident,
    renamed_serde_serialize: &Ident,
) -> Vec<Attribute> {
    parse_quote! {
        #[derive(#renamed_clap_parser)]
        #[derive(#renamed_serde_deserialize)]
        #[derive(#renamed_serde_serialize)]
    }
}

fn update_field_attrs(struct_type: &Ident, struct_fields: &mut Fields) {
    for field in struct_fields.iter_mut() {
        field.attrs = replace_doc_comment_with_clap_attr(
            struct_type,
            &field.ident.clone().unwrap(),
            &field.attrs,
        );
    }
}

fn replace_doc_comment_with_clap_attr(
    struct_type: &Ident,
    field: &Ident,
    attrs: &[Attribute],
) -> Vec<Attribute> {
    let mut result = Vec::new();
    let mut help_lines = String::new();
    for attr in attrs {
        if let Some(exiting_attr) = handle_existing_clap_attr(attr) {
            result.push(exiting_attr);
        } else if let Some(doc_comment) = handle_doc_comment(attr) {
            help_lines.push_str(&doc_comment);
        } else {
            result.push(attr.clone());
        }
    }
    if !help_lines.is_empty() {
        result.push(generate_clap_arg_help_attr(struct_type, field, &help_lines));
        result.push(generate_clap_arg_long_help_attr(
            struct_type,
            field,
            &help_lines,
        ));
    }

    result
}

fn handle_existing_clap_attr(attr: &Attribute) -> Option<Attribute> {
    if attr.path().is_ident("arg") {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("help") {
                unimplemented!("support for help attributes, please use doc comments");
            } else if meta.path.is_ident("long_help") {
                unimplemented!("support for long_help attributes, please use doc comments");
            }
            Ok(())
        });
        return Some(attr.clone());
    }

    None
}

fn handle_doc_comment(attr: &Attribute) -> Option<String> {
    if attr.path().is_ident("doc") {
        let doc_comment: Option<String> = match attr.meta {
            Meta::NameValue(ref name_value) => match name_value.value {
                Expr::Lit(ref lit) => match lit.lit {
                    Lit::Str(ref str) => Some(str.value() + "\n"),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        };

        return Some(
            doc_comment
                .unwrap()
                .strip_prefix(" ")
                .unwrap_or("\n")
                .to_string(),
        );
    }

    None
}

fn generate_clap_arg_help_attr(struct_type: &Ident, field: &Ident, help_lines: &str) -> Attribute {
    let help_string = help_lines
        .split_terminator('\n')
        .next()
        .unwrap()
        .to_string()
        + " [default: {:#?}]";

    parse_quote! {
        #[arg(help = format!(#help_string, <#struct_type>::default().#field.unwrap()))]
    }
}

fn generate_clap_arg_long_help_attr(
    struct_type: &Ident,
    field: &Ident,
    help_lines: &str,
) -> Attribute {
    let single_newline_re = Regex::new(
        r"(?<last_word_char_before_newline>\w)(\n{1})(?<first_word_char_after_newline>\w)",
    )
    .unwrap();
    let help_text = single_newline_re.replace_all(
        help_lines,
        "$last_word_char_before_newline $first_word_char_after_newline",
    );
    let help_string = help_text.trim_end_matches('\n').to_string() + "\n\n[default: {:#?}]";

    parse_quote! {
        #[arg(long_help = format!(#help_string, <#struct_type>::default().#field.unwrap()))]
    }
}

fn generate_impl_block(
    struct_type: &Ident,
    struct_fields: &Fields,
    alt_conf_file: &str,
) -> TokenStream {
    let mut result = Vec::new();
    result.push(generate_parse_all_sources_fn(struct_type));
    result.push(generate_static_conf_file_path_fn(alt_conf_file));
    result.push(generate_figment_to_config_fn(struct_type));
    result.push(generate_figment_with_defaults_fn(struct_type));
    result.push(generate_figment_new_fn());
    result.push(generate_figment_defaults_merge_fn(struct_type));
    result.push(generate_figment_conf_file_merge_fn());
    result.push(generate_figment_env_var_merge_fn());
    result.push(generate_figment_extract_fn(struct_type));
    result.push(generate_clap_to_config_fn(struct_type));
    result.push(generate_clap_parse_fn(struct_type));
    result.push(generate_clap_merge_fn(struct_type, struct_fields));
    result.push(generate_parse_extra_conf_file_fn(struct_type));
    result.push(generate_figment_to_config_with_extra_conf_file_fn(
        struct_type,
    ));
    result.push(generate_clap_to_existing_config_fn(struct_type));
    result.push(generate_clap_merge_existing_fn(struct_type, struct_fields));

    quote! {
        impl #struct_type {
            #(#result)*
        }
    }
}

fn generate_parse_all_sources_fn(struct_type: &Ident) -> TokenStream {
    quote! {
        fn parse_all_sources() -> #struct_type {
            let config = <#struct_type>::figment_to_config();
            <#struct_type>::clap_to_config(config)
        }
    }
}

fn generate_figment_to_config_fn(struct_type: &Ident) -> TokenStream {
    quote! {
        fn figment_to_config() -> #struct_type {
            let mut config = <#struct_type>::figment_with_defaults();
            config = <#struct_type>::figment_conf_file_merge(
                config,
                &<#struct_type>::static_conf_file_path()
            );
            config = <#struct_type>::figment_env_var_merge(config);
            <#struct_type>::figment_extract(config)
        }
    }
}

fn generate_static_conf_file_path_fn(alt_conf_file: &str) -> TokenStream {
    let mut result = Vec::new();
    if alt_conf_file.is_empty() {
        result.push(quote! {
            conf_file.set_extension("toml");
        });
    } else {
        result.push(quote! {
            conf_file.set_file_name(#alt_conf_file);
        });
    }

    quote! {
        fn static_conf_file_path() -> std::path::PathBuf {
            let mut conf_file = std::path::PathBuf::from(file!());
            #(#result)*

            conf_file
        }
    }
}

fn generate_figment_with_defaults_fn(struct_type: &Ident) -> TokenStream {
    quote! {
        fn figment_with_defaults() -> figment::Figment {
            let config = <#struct_type>::figment_new();
            <#struct_type>::figment_defaults_merge(config)
        }
    }
}

fn generate_figment_new_fn() -> TokenStream {
    quote! {
        fn figment_new() -> figment::Figment {
            figment::Figment::new()
        }
    }
}

fn generate_figment_defaults_merge_fn(struct_type: &Ident) -> TokenStream {
    quote! {
        fn figment_defaults_merge(config: figment::Figment) -> figment::Figment {
            config.merge(figment::providers::Serialized::defaults(<#struct_type>::default()))
        }
    }
}

fn generate_figment_conf_file_merge_fn() -> TokenStream {
    quote! {
        fn figment_conf_file_merge(
            mut config: figment::Figment,
            conf_file: &std::path::PathBuf
        ) -> figment::Figment {
            config = config.merge(figment::providers::Toml::file(conf_file));

            config
        }
    }
}

fn generate_figment_env_var_merge_fn() -> TokenStream {
    quote! {
        fn figment_env_var_merge(config: figment::Figment) -> figment::Figment {
            config.merge(figment::providers::Env::prefixed("GWR_"))
        }
    }
}

fn generate_figment_extract_fn(struct_type: &Ident) -> TokenStream {
    quote! {
        fn figment_extract(config: figment::Figment) -> #struct_type {
            config.extract().unwrap()
        }
    }
}

fn generate_clap_to_config_fn(struct_type: &Ident) -> TokenStream {
    quote! {
        fn clap_to_config(config: #struct_type) -> #struct_type {
            let cli: #struct_type = <#struct_type>::clap_parse();
            <#struct_type>::clap_merge(config, cli)
        }
    }
}

fn generate_clap_parse_fn(struct_type: &Ident) -> TokenStream {
    quote! {
        fn clap_parse() -> #struct_type {
            <#struct_type>::parse()
        }
    }
}

fn generate_clap_merge_fn(struct_type: &Ident, struct_fields: &Fields) -> TokenStream {
    let mut result = Vec::new();
    for field in struct_fields {
        let field = format_ident!("{}", field.ident.clone().unwrap().to_string());
        result.push(quote! {
            if cli.#field.is_some() {
                config.#field = cli.#field;
            }
        });
    }

    quote! {
        fn clap_merge(mut config: #struct_type, cli: #struct_type) -> #struct_type {
            #(#result)*

            config
        }
    }
}

fn generate_parse_extra_conf_file_fn(struct_type: &Ident) -> TokenStream {
    quote! {
        fn parse_extra_conf_file(
            &mut self,
            conf_file: &std::path::PathBuf
        ) -> Result<(), std::io::Error> {
            if conf_file.as_os_str() == "" {
                return Ok(())
            }

            if conf_file.is_dir() {
                return Err(
                    std::io::Error::new(
                        std::io::ErrorKind::IsADirectory,
                        format!("{} is not a file path", conf_file.display())
                    )
                );
            }

            if !conf_file.exists() {
                return Err(
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("{} not found", conf_file.display())
                    )
                );
            }

            let mut config = <#struct_type>::figment_to_config_with_extra_conf_file(conf_file);
            self.clap_to_existing_config(config);

            Ok(())
        }
    }
}

fn generate_figment_to_config_with_extra_conf_file_fn(struct_type: &Ident) -> TokenStream {
    quote! {
        fn figment_to_config_with_extra_conf_file(conf_file: &std::path::PathBuf) -> #struct_type {
            let mut figment = <#struct_type>::figment_with_defaults();
            figment = <#struct_type>::figment_conf_file_merge(
                figment,
                &<#struct_type>::static_conf_file_path()
            );
            figment = <#struct_type>::figment_conf_file_merge(figment, conf_file);
            figment = <#struct_type>::figment_env_var_merge(figment);
            <#struct_type>::figment_extract(figment)
        }
    }
}

fn generate_clap_to_existing_config_fn(struct_type: &Ident) -> TokenStream {
    quote! {
        fn clap_to_existing_config(&mut self, config: #struct_type) {
            let cli: #struct_type = <#struct_type>::clap_parse();
            self.clap_merge_existing(config, cli);
        }
    }
}

fn generate_clap_merge_existing_fn(struct_type: &Ident, struct_fields: &Fields) -> TokenStream {
    let mut result = Vec::new();
    for field in struct_fields {
        let field = format_ident!("{}", field.ident.clone().unwrap().to_string());
        result.push(quote! {
            if cli.#field.is_some() {
                self.#field = cli.#field;
            } else if config.#field != #struct_type::default().#field {
                self.#field = config.#field;
            }
        });
    }

    quote! {
        fn clap_merge_existing(&mut self, config: #struct_type, cli: #struct_type) {
            #(#result)*
        }
    }
}
