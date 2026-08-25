// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

#![doc(test(attr(deny(unused_must_use))))]
#![doc = std::include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]

extern crate proc_macro;
use proc_macro::TokenStream;
use syn::{LitStr, parse_macro_input};

mod multi_source_config;

/// Enables an annotated struct to be used for configuration of an application.
///
/// The macro generates a `parse_all_sources()` function for the annotated
/// structure, which the application must call to parse all configuration
/// sources and merge any values set (as well as any default values defined
/// within the application source). This function returns an instantiated object
/// of the annotated struct type.
/// ```rust
/// # use gwr_config::multi_source_config;
/// #
/// #[multi_source_config]
/// struct Config {
///     log_level: Option<String>,
/// }
/// #
/// # impl Default for Config {
/// #     fn default() -> Self {
/// #         Self {
/// #             log_level: Some("warn".to_string()),
/// #         }
/// #     }
/// # }
///
/// fn main() {
///     let config = Config::parse_all_sources();
/// #    println!("log_level: {}", config.log_level.unwrap());
/// }
/// ```
///
/// The macro also generates a
/// `parse_extra_conf_file(&mut self, conf_file: &std::path::PathBuf) ->
/// Result<(), std::io::Error>` method, which applications can call to update
/// the state of a configuration struct with values from a TOML file. This
/// allows the filename to be given at runtime if desired.
///
/// # Dependencies
///
/// ## Crates
///
/// Applications using ths macro must add `gwr-config`, `clap` (with the
/// `derive` feature enabled), `figment` (with the `env` and `toml` features
/// enabled), and `serde` (with the `derive` feature enabled), to the
/// `dependencies` table of their Cargo.toml, for example:
/// ```toml
/// [dependencies]
/// clap = { version = "4.4.6", features = ["derive"] }
/// figment = { version = "0.10.19", features = ["env", "toml"] }
/// serde = { version = "1.0.188", features = ["derive"] }
/// gwr-config = { path = "../gwr-config" }
/// ```
///
/// ## Use statements
///
/// Before the [macro@multi_source_config] attribute can be used the
/// `use gwr_config::multi_source_config;` statement must be present. Use
/// statements for [Clap], [Figment], and [Serde] are not required as the macro
/// will emit these (with customised names to avoid the risk of a clash),
/// however they can be present if required by other parts of the source code.
///
/// # Priority of sources
///
/// Where the same piece of configuration is supplied by multiple sources the
/// following priority order will be applied:
/// 1. [Command-line interface](#command-line-interface).
/// 1. [Environment variable](#environment-variables).
/// 1. [Run-time configuration files](#run-time-configuration-files).
/// 1. [Compile-time configuration file](#compile-time-configuration-file).
/// 1. [Default value](#default-values) (specified within the application source
///    code).
///
/// # Option values
///
/// The macro requires that all fields of the configuration struct are wrapped
/// in the `Option` type. This allows the merging together of parsed results
/// from [clap_derive] and [Figment] without [clap_derive] requiring that
/// default value attributes be defined on each field.
///
/// See [Default values](#default-values) for futher details.
///
/// # Command-line interface
///
/// ## Help messages
///
/// As is standard practise when using
/// [clap_derive] the help messages displayed when `-h` is passed to the CLI
/// (and the long help messages shown when `--help` is passed) are supplied as
/// `///` doc comments for each field of the struct. Unlike when using
/// [clap_derive] directly the magic attribute `help` and `long_help` cannot be
/// supplied as Arg attributes on fields of the struct. For example:
/// ```rust
/// # use gwr_config::multi_source_config;
/// #
/// #[multi_source_config]
/// struct Config {
///     /// Configure the logging level for the log messages
///     log_level: Option<String>,
/// }
/// #
/// # impl Default for Config {
/// #     fn default() -> Self {
/// #         Self {
/// #             log_level: Some("warn".to_string()),
/// #         }
/// #     }
/// # }
/// #
/// # fn main() {
/// #     let config = Config::parse_all_sources();
/// #     println!("log_level: {}", config.log_level.unwrap());
/// # }
/// ```
///
/// ## Magic attributes
///
/// Other [clap_derive] Arg attributes can still be used e.g. to specify whether
/// a field should have a long, short, or both names when exposed via the CLI.
///
/// # Environment variables
///
/// For use as environment variables the field names will be prefixed with
/// `GWR_`, with [Figment] handling any case conversions as necessary.
/// For example, for the following configuration struct:
/// ```rust
/// # use gwr_config::multi_source_config;
/// #
/// #[multi_source_config]
/// struct Config {
///     /// Configure the logging level for the log messages
///     log_level: Option<String>,
/// }
/// #
/// # impl Default for Config {
/// #     fn default() -> Self {
/// #         Self {
/// #             log_level: Some("warn".to_string()),
/// #         }
/// #     }
/// # }
/// #
/// # fn main() {
/// #     let config = Config::parse_all_sources();
/// #     println!("log_level: {}", config.log_level.unwrap());
/// # }
/// ```
/// The `log_level` field could be set via the `GWR_LOG_LEVEL` environment
/// variable.
///
/// # Configuration files
///
/// Configuration files must be in the [TOML] format, and can be provided both
/// dynamically at run-time and statically at compile-time.
///
/// The requirement to use `Option` types within the configuration struct also
/// allows the use of "partial" configuration files, i.e. files which only
/// specify a subset of the applications configuration.
///
/// For example, if an application defined its configuration struct in a source
/// file named `main.rs` then the macro would attempt to read configuration from
/// `main.toml`.
///
/// Where main.rs contains:
/// ```rust
/// # use gwr_config::multi_source_config;
/// #
/// #[multi_source_config]
/// struct Config {
///     /// Configure the logging level for the log messages
///     log_level: Option<String>,
///
///     /// Enable trace events
///     #[arg(long)]
///     enable_trace: Option<bool>,
/// }
/// #
/// # impl Default for Config {
/// #     fn default() -> Self {
/// #         Self {
/// #             log_level: Some("warn".to_string()),
/// #             enable_trace: Some(Default::default()),
/// #         }
/// #     }
/// # }
/// #
/// # fn main() {
/// #     let config = Config::parse_all_sources();
/// #     println!("log_level: {}", config.log_level.unwrap());
/// #     println!("enable_trace: {}", config.enable_trace.unwrap());
/// # }
/// ```
///
/// And a partial main.toml could, for example, contain:
/// ```toml
/// log_level = "info"
/// ```
///
/// Whereas a complete main.toml for this example would be:
/// ```toml
/// log_level = "info"
/// enable_trace = true
/// ```
///
/// ## Run-time configuration files
///
/// Should an application wish to accept configuration file paths at run-time a
/// second parser pass is required. The first pass allows the application to
/// pick up the desired configuration file path, and the second pass will merge
/// the values found in this file into the configuration object. To do this the
/// `parse_extra_conf_file(&mut self, conf_file: &std::path::PathBuf) ->
/// Result<(), std::io::Error>` method is provided.
/// ```rust
/// # use std::path::PathBuf;
/// # use gwr_config::multi_source_config;
/// #
/// #[multi_source_config]
/// struct Config {
///     /// Configure the logging level for the log messages
///     log_level: Option<String>,
///
///     /// Enable trace events
///     #[arg(long)]
///     enable_trace: Option<bool>,
///
///     /// Path to additional configuration file
///     ///
///     /// This additional configuration file must contain TOML, and set values for
///     /// fields of this struct.
///     #[arg(long)]
///     conf_file: Option<PathBuf>,
/// }
/// #
/// # impl Default for Config {
/// #     fn default() -> Self {
/// #         Self {
/// #             log_level: Some("warn".to_string()),
/// #             enable_trace: Some(Default::default()),
/// #             conf_file: Some(Default::default()),
/// #         }
/// #     }
/// # }
///
/// fn main() -> Result<(), std::io::Error> {
///     let mut config = Config::parse_all_sources();
///     let extra_conf_file = config.conf_file.clone().unwrap();
///     config.parse_extra_conf_file(&extra_conf_file)?;
/// #     println!("log_level: {}", config.log_level.unwrap());
/// #     println!("enable_trace: {}", config.enable_trace.unwrap());
/// #     Ok(())
/// }
/// ```
///
/// Multiple configuration files can be used to provide additional configuration
/// values at run-time by simply calling `parse_extra_conf_file()` with each
/// file. Where the same field is set in more that one of these configuration
/// files the value will be taken from the last file to be parsed.
///
/// ## Compile-time configuration file
///
/// By default the compile-time configuration file path will be such that files
/// named the same as the source file that the macro is invoked within, but with
/// the extension `.toml` will be used.
///
/// Should an alternative configuration file path be desired the `conf_file`
/// property can be can be given as an input to the macro, setting the path at
/// compile-time.
/// ```rust
/// # use gwr_config::multi_source_config;
/// #
/// #[multi_source_config(conf_file = "app_conf.toml")]
/// struct Config {
///     /// Configure the logging level for the log messages
///     log_level: Option<String>,
///
///     /// Enable trace events
///     #[arg(long)]
///     enable_trace: Option<bool>,
/// }
/// #
/// # impl Default for Config {
/// #     fn default() -> Self {
/// #         Self {
/// #             log_level: Some("warn".to_string()),
/// #             enable_trace: Some(Default::default()),
/// #         }
/// #     }
/// # }
/// #
/// # fn main() {
/// #     let config = Config::parse_all_sources();
/// #     println!("log_level: {}", config.log_level.unwrap());
/// #     println!("enable_trace: {}", config.enable_trace.unwrap());
/// # }
/// ```
///
/// ## Missing configuration files
///
/// How a missing configuration file is handled depends on whether it is a path
/// set at compile-time or run-time.
///
/// No error will be raised when a compile-time configuartion file is missing,
/// with the parse and merge of this source being silently skipped. In contrast
/// to this, when `parse_extra_conf_file()` is called with a missing or invalid
/// file path an IO error will be returned and the configuartion update process
/// aborted.
///
/// Empty paths (`""`) will be treated as valid, and therefore no error will be
/// returned, but no configuration update process will be performed.
///
/// # Default values
///
/// Default values for configuration fields must not be provided via the
/// [clap_derive] `default_value`, `default_value_t`, `default_values_t`,
/// `default_value_os_t`, or `default_values_os_t` magic attributes. Instead, a
/// `Default` implementation needs to be provided as this allows the macro to
/// ensure the default value for a field is only used once all configuration
/// sources have been parsed.
/// ```rust
/// # use gwr_config::multi_source_config;
/// #
/// #[multi_source_config]
/// struct Config {
///     /// Configure the logging level for the log messages
///     log_level: Option<String>,
///
///     /// Enable trace events
///     #[arg(long)]
///     enable_trace: Option<bool>,
/// }
///
/// impl Default for Config {
///     fn default() -> Self {
///         Self {
///             log_level: Some("warn".to_string()),
///             enable_trace: Some(Default::default()),
///         }
///     }
/// }
/// #
/// # fn main() {
/// #     let config = Config::parse_all_sources();
/// #     println!("log_level: {}", config.log_level.unwrap());
/// #     println!("enable_trace: {}", config.enable_trace.unwrap());
/// # }
/// ```
///
/// # Complete examples
///
/// ## Default compile-time configuration file path
///
/// ```rust
/// use gwr_config::multi_source_config;
///
/// #[multi_source_config]
/// struct Config {
///     /// Configure the logging level for the log messages
///     log_level: Option<String>,
///
///     /// Enable trace events
///     #[arg(long)]
///     enable_trace: Option<bool>,
/// }
///
/// impl Default for Config {
///     fn default() -> Self {
///         Self {
///             log_level: Some("warn".to_string()),
///             enable_trace: Some(Default::default()),
///         }
///     }
/// }
///
/// # #[allow(clippy::needless_doctest_main)]
/// fn main() {
///     let config = Config::parse_all_sources();
///     println!("log_level: {}", config.log_level.unwrap());
///     println!("enable_trace: {}", config.enable_trace.unwrap());
/// }
/// ```
///
/// ## Alternative compile-time configuration file path
///
/// ```rust
/// use gwr_config::multi_source_config;
///
/// #[multi_source_config(conf_file = "app_conf.toml")]
/// struct Config {
///     /// Configure the logging level for the log messages
///     log_level: Option<String>,
///
///     /// Enable trace events
///     #[arg(long)]
///     enable_trace: Option<bool>,
/// }
///
/// impl Default for Config {
///     fn default() -> Self {
///         Self {
///             log_level: Some("warn".to_string()),
///             enable_trace: Some(Default::default()),
///         }
///     }
/// }
///
/// # #[allow(clippy::needless_doctest_main)]
/// fn main() {
///     let config = Config::parse_all_sources();
///     println!("log_level: {}", config.log_level.unwrap());
///     println!("enable_trace: {}", config.enable_trace.unwrap());
/// }
/// ```
///
/// ## Run-time configuration file path
///
/// ```rust
/// use std::path::PathBuf;
///
/// use gwr_config::multi_source_config;
///
/// #[multi_source_config]
/// struct Config {
///     /// Configure the logging level for the log messages
///     log_level: Option<String>,
///
///     /// Enable trace events
///     #[arg(long)]
///     enable_trace: Option<bool>,
///
///     /// Path to additional configuration file
///     ///
///     /// This additional configuration file must contain TOML, and set values for
///     /// fields of this struct.
///     #[arg(long)]
///     conf_file: Option<PathBuf>,
/// }
///
/// impl Default for Config {
///     fn default() -> Self {
///         Self {
///             log_level: Some("warn".to_string()),
///             enable_trace: Some(Default::default()),
///             conf_file: Some(Default::default()),
///         }
///     }
/// }
///
/// fn main() -> Result<(), std::io::Error> {
///     let mut config = Config::parse_all_sources();
///     let extra_conf_file = config.conf_file.clone().unwrap();
///     config.parse_extra_conf_file(&extra_conf_file)?;
///     println!("log_level: {}", config.log_level.unwrap());
///     println!("enable_trace: {}", config.enable_trace.unwrap());
///
///     Ok(())
/// }
/// ```
///
/// [Clap]: https://docs.rs/clap/latest/clap/
/// [clap_derive]: https://docs.rs/clap/latest/clap/_derive/index.html
/// [Figment]: https://docs.rs/figment/latest/figment/
/// [Serde]: https://docs.rs/serde/latest/serde/
/// [TOML]: https://toml.io/en/
#[proc_macro_attribute]
pub fn multi_source_config(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut alt_conf_file = String::default();
    let attr_parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("conf_file") {
            alt_conf_file = meta.value()?.parse::<LitStr>()?.value();
            Ok(())
        } else {
            Err(meta.error("unsupported property"))
        }
    });
    parse_macro_input!(attr with attr_parser);

    let item = parse_macro_input!(item);

    multi_source_config::multi_source_config_impl(&alt_conf_file, item).into()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use serial_test::serial;

    fn get_workspace_dir() -> PathBuf {
        let package_dir = std::env::current_dir().unwrap();
        package_dir.parent().unwrap().to_path_buf()
    }

    #[test]
    #[serial(no_conf_file)]
    fn defaults_with_no_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "no_conf_file",
                ])
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"\",
    ),
    b: Some(
        false,
    ),
    c: Some(
        0,
    ),
    d: Some(
        \"foo\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(no_conf_file)]
    fn defaults_and_cli_with_no_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "no_conf_file",
                ])
                .arg("--")
                .args(["--a", "bar"])
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"bar\",
    ),
    b: Some(
        false,
    ),
    c: Some(
        0,
    ),
    d: Some(
        \"foo\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(no_conf_file)]
    fn defaults_and_env_vars_with_no_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "no_conf_file",
                ])
                .env("GWR_B", "true")
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"\",
    ),
    b: Some(
        true,
    ),
    c: Some(
        0,
    ),
    d: Some(
        \"foo\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(no_conf_file)]
    fn defaults_and_cli_and_env_vars_with_no_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "no_conf_file",
                ])
                .arg("--")
                .args(["--a", "bar"])
                .env("GWR_B", "true")
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"bar\",
    ),
    b: Some(
        true,
    ),
    c: Some(
        0,
    ),
    d: Some(
        \"foo\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(partial_conf_file)]
    fn defaults_and_partial_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "partial_conf_file",
                ])
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"\",
    ),
    b: Some(
        false,
    ),
    c: Some(
        42,
    ),
    d: Some(
        \"foo\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(partial_conf_file)]
    fn defaults_and_cli_and_partial_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "partial_conf_file",
                ])
                .arg("--")
                .args(["--a", "bar"])
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"bar\",
    ),
    b: Some(
        false,
    ),
    c: Some(
        42,
    ),
    d: Some(
        \"foo\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(partial_conf_file)]
    fn defaults_and_env_vars_and_partial_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "partial_conf_file",
                ])
                .env("GWR_B", "true")
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"\",
    ),
    b: Some(
        true,
    ),
    c: Some(
        42,
    ),
    d: Some(
        \"foo\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(partial_conf_file)]
    fn defaults_and_cli_and_env_vars_and_partial_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "partial_conf_file",
                ])
                .arg("--")
                .args(["--a", "bar"])
                .env("GWR_B", "true")
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"bar\",
    ),
    b: Some(
        true,
    ),
    c: Some(
        42,
    ),
    d: Some(
        \"foo\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(full_conf_file)]
    fn defaults_and_full_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "full_conf_file",
                ])
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"bee\",
    ),
    b: Some(
        true,
    ),
    c: Some(
        42,
    ),
    d: Some(
        \"bar\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(full_conf_file)]
    fn defaults_and_cli_and_full_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "full_conf_file",
                ])
                .arg("--")
                .args(["--a", "bar"])
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"bar\",
    ),
    b: Some(
        true,
    ),
    c: Some(
        42,
    ),
    d: Some(
        \"bar\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(full_conf_file)]
    fn defaults_and_env_vars_and_full_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "full_conf_file",
                ])
                .env("GWR_C", "64")
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"bee\",
    ),
    b: Some(
        true,
    ),
    c: Some(
        64,
    ),
    d: Some(
        \"bar\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(full_conf_file)]
    fn defaults_and_cli_and_env_vars_and_full_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "full_conf_file",
                ])
                .arg("--")
                .args(["--a", "fee"])
                .env("GWR_C", "64")
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"fee\",
    ),
    b: Some(
        true,
    ),
    c: Some(
        64,
    ),
    d: Some(
        \"bar\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(partial_conf_file)]
    fn defaults_and_cli_and_env_vars_and_conf_file_priority() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "partial_conf_file",
                ])
                .arg("--")
                .args(["--c", "96"])
                .env("GWR_C", "64")
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"\",
    ),
    b: Some(
        false,
    ),
    c: Some(
        96,
    ),
    d: Some(
        \"foo\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(alt_conf_file)]
    fn defaults_and_alternative_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "alt_conf_file",
                ])
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"\",
    ),
    b: Some(
        false,
    ),
    c: Some(
        42,
    ),
    d: Some(
        \"foo\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(alt_conf_file_missing)]
    fn defaults_and_missing_alternative_conf_file() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "alt_conf_file_missing",
                ])
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"\",
    ),
    b: Some(
        false,
    ),
    c: Some(
        0,
    ),
    d: Some(
        \"foo\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(alt_and_runtime_conf_files)]
    fn defaults_and_alternative_and_extra_conf_files() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "alt_and_runtime_conf_files",
                ])
                .arg("--")
                .args(["--conf-file", "gwr-config/examples/runtime_conf_file.toml"])
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"\",
    ),
    b: Some(
        false,
    ),
    c: Some(
        88,
    ),
    d: Some(
        \"foo\",
    ),
    conf_file: Some(
        \"gwr-config/examples/runtime_conf_file.toml\",
    ),
}
";

        assert_eq!(run, expected);
    }

    #[test]
    #[serial(alt_and_runtime_conf_files)]
    fn defaults_and_cli_and_env_vars_and_alt_and_extra_conf_files_priority() {
        let run = String::from_utf8(
            Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "gwr-config",
                    "--example",
                    "alt_and_runtime_conf_files",
                ])
                .arg("--")
                .args([
                    "--c",
                    "96",
                    "--conf-file",
                    "gwr-config/examples/runtime_conf_file.toml",
                ])
                .env("GWR_C", "64")
                .current_dir(get_workspace_dir())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let expected = "Config {
    a: Some(
        \"\",
    ),
    b: Some(
        false,
    ),
    c: Some(
        96,
    ),
    d: Some(
        \"foo\",
    ),
    conf_file: Some(
        \"gwr-config/examples/runtime_conf_file.toml\",
    ),
}
";

        assert_eq!(run, expected);
    }
}
