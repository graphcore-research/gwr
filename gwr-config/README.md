<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-config

The `gwr_config` library provides a hierarchical configuration mechanism for
applications which wish to accept settings and options from configuration files
(with support for both compile-time and run-time file paths), environment
variables, and as a command-line interface.

A single macro, `multi_source_config`, is provided which can be applied to a
struct that represents the configuration the application can accept. This macro
combines the features of
[clap_derive](https://docs.rs/clap/latest/clap/_derive/index.html) and
[Figment](https://docs.rs/figment/latest/figment/), and its use is largely
similar to working with them.

## Example

```rust
# use std::path::PathBuf;
use gwr_config::multi_source_config;

#[multi_source_config(conf_file = "app_conf.toml")]
#[derive(Debug)]
#[command(about = "multi_source_config example application.")]
struct Config {
    /// Configure the logging level for the log messages
    #[arg(long)]
    log_level: Option<String>,

    /// Enable trace events
    #[arg(long)]
    enable_trace: Option<bool>,

    /// Specify a log file to write text log/trace to.
    ///
    /// Use '-' to write to stdout.
    #[arg(short = 'l', long = "log-file")]
    log_file: Option<String>,

    /// Path to additional configuration file
    ///
    /// This additional configuration file must contain TOML, and set values
    /// for fields of this struct.
    #[arg(long)]
    conf_file: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: Some("warn".to_string()),
            enable_trace: Some(Default::default()),
            log_file: Some("-".to_string()),
            conf_file: Some(Default::default()),
        }
    }
}

# #[allow(clippy::needless_doctest_main)]
fn main() -> Result<(), std::io::Error> {
    let mut config = Config::parse_all_sources();
    let extra_conf_file = config.conf_file.clone().unwrap();
    config.parse_extra_conf_file(&extra_conf_file)?;
    println!("config: {:#?}", config);
    Ok(())
}
```

Running the above application and passing the `--help` argument results in the
following output:

```text
multi_source_config example application.

Usage: multi_source_config_example [OPTIONS]

Options:
      --log-level <LOG_LEVEL>
          Configure the logging level for the log messages

          [default: warn]

      --enable-trace <ENABLE_TRACE>
          Enable trace events

          [default: false]

          [possible values: true, false]

  -l, --log-file <LOG_FILE>
          Specify a log file to write text log/trace to.

          Use '-' to write to stdout.

          [default: "-"]

      --conf-file <CONF_FILE>
         Path to additional configuration file

         This additional configuration file must contain TOML, and set values for fields of this struct.

         [default: ""]

  -h, --help
          Print help (see a summary with '-h')
```

Instead of passing command-line arguments the configuration can be controlled by
environment variables. For the above example these would be `GWR_LOG_LEVEL`,
`GWR_ENABLE_TRACE`, `GWR_LOG_FILE`, and `GWR_CONF_FILE`.

A configuration file can also be used to control these application settings:

```toml
log_level = "info"
enable_trace = true
log_file = "example.log"
conf_file = "app_conf.toml"
```

## Debug

The macro attempts to detect unsupported use cases, panicking with a message
that should help to identify the issue quickly.

For cases where this is not possible, the `macro-backtrace` feature and the
`cargo-expand` plugin can be helpful in understanding what is going wrong:

- `RUSTFLAGS="-Zmacro-backtrace" cargo +nightly run -p gwr-track --example log_test`
- `cargo expand -p gwr-track --example log_test`
