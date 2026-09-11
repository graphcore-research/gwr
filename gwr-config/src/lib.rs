// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

#![doc(test(attr(deny(unused_must_use))))]
#![doc = std::include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]

extern crate proc_macro;
use proc_macro::TokenStream;
use syn::{LitStr, parse_macro_input};

mod contract;
mod multi_source_config;

/// Generate application configuration with precedence CLI > `GWR_` environment
/// > TOML file > typed Rust defaults.
///
/// This is deliberately **not** a general Clap/Figment compatibility layer.
/// Only the following subset is supported; other Clap/Serde attributes on the
/// configuration struct and its fields produce a compile error.
///
/// # Supported subset
///
/// - Named, non-generic structs; ordinary scalar types, `Option<T>`, and
///   `Vec<T>` supported by Clap and Serde. Fields must use these types
///   directly, not aliases hiding `Option` or `Vec`.
/// - `#[arg(...)]` (or `#[clap(...)]`): `short`, `long`, explicit
///   `default_value_t = expression`, `value_parser = expression`, `value_enum`,
///   and literal `required = true/false` on `Vec<T>` fields without defaults.
///   Explicit `long = "name"` values must be string literals. No other argument
///   properties.
/// - `#[command(about = "...")]`; documentation comments supply argument help.
/// - Paired `#[command(flatten)]` and `#[serde(flatten)]` for another annotated
///   named type. Import both `Child` and generated `ChildPartial` when needed.
///   TOML and environment keys remain flat, using Rust field names. Configure
///   file selection only on the root: flattened child types must not declare
///   `conf_file_flag` (their file defaults are not loaded separately).
/// - Leaf-struct `#[group(id = "...", multiple = false)]` for mutually
///   exclusive alternatives (such as Spotter's log/bin input). A
///   higher-priority selection replaces the whole lower-priority group. Group
///   fields must be `Option<T>` without defaults, with no flattening. Put
///   `conf_file_flag` on the parent, not on an exclusive group. `multiple =
///   true` disables exclusivity.
/// - Field `#[serde(default)]` and `#[serde(deserialize_with = "...")]`.
///   `deserialize_with` requires an `Option<T>` field and must deserialize that
///   optional type, leaving missing values absent until fallback application.
///   Serde attributes on a field's own type, such as enum renaming, are
///   unaffected.
/// - Field `#[cfg(...)]` guards. `cfg_attr` is rejected to prevent hidden
///   metadata.
///
/// # Deliberate restrictions
///
/// Conditional/string/plural/OS defaults, `requires*`, `conflicts_with*`,
/// `required_if*`, subcommands, custom actions/IDs, argument-level groups/env,
/// aliases, delimiters, and Serde rename/alias/skip or unpaired flattening are
/// unsupported. Keep cross-field and domain validation in the application.
/// Do not derive `Parser`, `Args`, `Default`, `Serialize`, or `Deserialize` on
/// the annotated struct: the macro supplies the necessary generated traits.
///
/// `default_value_t = expr` is a typed fallback applied **after** source
/// merging; for `Option<T>`, `expr` has type `T`. Custom Clap parsers run only
/// for explicit CLI input, once per value. They do not validate defaults, TOML,
/// or environment values: validate those in the application's Serde
/// deserializer or final domain validation. Default expressions should be pure;
/// command construction evaluates them for help too, displaying `Debug` or
/// the `ValueEnum` spelling. Enum defaults without a CLI spelling (such as
/// `#[value(skip)]` variants) display `[default: non-CLI value]` instead;
/// their typed fallback is still applied after merging.
///
/// Boolean options accept `--flag`, `--flag=true`, and `--flag=false`.
/// An omitted `Option<bool>` remains `None` unless it has a typed fallback.
/// Every non-optional field without a fallback, including a `Vec<T>`, must be
/// present after merging; `required = true` additionally rejects empty vectors.
/// Missing required values, source
/// errors and merged group conflicts panic; malformed CLI input uses Clap's
/// diagnostics. This macro does not provide a fallible configuration API.
///
/// # Files and sources
///
/// `default_conf_file = "name.toml"` replaces the source filename; otherwise
/// the source file's extension becomes `.toml`. Relative paths are resolved
/// against the working directory. A missing implicit file is allowed.
/// `conf_file_flag = "conf-file"` adds `--conf-file PATH`. The generated flag
/// must not collide with an explicit or inferred field long option
/// (including flattened fields) or reserved `--help`; collisions are rejected
/// at compile time. Inferred longs use Clap's kebab-case naming.
/// An explicit missing file or directory is an error, and an empty path
/// disables file loading.
/// Environment variables have fixed prefix `GWR_`, with case-insensitive field
/// names. Higher-priority values replace entire scalar/vector fields before
/// typed deserialization, so an overridden invalid field does not cause an
/// error. TOML syntax errors still fail; unknown configuration keys are
/// ignored.
///
/// Call `Config::parse_all_sources()` (or `parse_all_sources_with_matches` with
/// matches from **`ConfigPartial::command()`**) for layered parsing. The
/// generated `ConfigPartial` is sparse and contains no typed fallbacks. Direct
/// Clap/Serde parsing of the final struct is not the layered API. Defaultable
/// leaf structs retain `Default` and `clap::Args` for reusable application
/// options.
///
/// Applications need `clap` with `derive`, `serde` with `derive`,
/// and `figment` with `env` and `toml` alongside `gwr-config`.
///
/// ```no_run
/// use gwr_config::multi_source_config;
///
/// #[multi_source_config(conf_file_flag = "conf-file")]
/// #[derive(Debug)]
/// struct Config {
///     /// Number of workers.
///     #[arg(long, default_value_t = 4)]
///     workers: usize,
///     #[arg(long)]
///     verbose: Option<bool>,
/// }
/// let config = Config::parse_all_sources();
/// assert!(config.workers > 0);
/// ```
#[proc_macro_attribute]
pub fn multi_source_config(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut default_conf_file = String::default();
    let mut conf_file_flag = None;
    let attr_parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("default_conf_file") {
            default_conf_file = meta.value()?.parse::<LitStr>()?.value();
            Ok(())
        } else if meta.path.is_ident("conf_file_flag") {
            conf_file_flag = Some(meta.value()?.parse::<LitStr>()?.value());
            Ok(())
        } else {
            Err(meta.error("unsupported property"))
        }
    });
    parse_macro_input!(attr with attr_parser);
    let item = parse_macro_input!(item);
    multi_source_config::multi_source_config_impl(
        &default_conf_file,
        conf_file_flag.as_deref(),
        item,
    )
    .into()
}
