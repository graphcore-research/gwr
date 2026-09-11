// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::io;
use std::path::PathBuf;
#[cfg(feature = "perfetto")]
use std::process::exit;

use clap::CommandFactory;
use gwr_config::multi_source_config;
use gwr_spotter::app::{App, AppResult};
use gwr_spotter::event::{Event, EventHandler};
use gwr_spotter::handler::handle_key_events;
use gwr_spotter::http_server::spawn;
#[cfg(feature = "perfetto")]
use gwr_spotter::perfetto;
use gwr_spotter::tui::Tui;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// Input subcommand arguments.
#[multi_source_config]
#[derive(Debug, PartialEq)]
#[group(id = "input", multiple = false)]
struct InputOptions {
    /// Provide a textual log file to be parsed
    #[arg(long)]
    log: Option<PathBuf>,

    /// Provide a capnp-based binary trace
    #[arg(long)]
    bin: Option<PathBuf>,
}

/// Command-line arguments.
#[multi_source_config]
#[derive(Debug, PartialEq)]
#[command(about = "GWR log/binary trace viewer")]
struct Cli {
    #[command(flatten)]
    #[serde(flatten)]
    input: InputOptions,

    /// Generate Perfetto output from GWR binary trace with this name
    ///
    /// gwr-spotter will exit having produced the Perfetto trace.
    #[cfg(feature = "perfetto")]
    #[arg(long)]
    perfetto: Option<PathBuf>,

    /// Run without spawning the loopback HTTP API
    ///
    /// The web frontend will be unavailable.
    #[arg(long, default_value_t = false)]
    no_server: Option<bool>,
}

impl Cli {
    fn normalize(mut self, log_overrides_perfetto: bool) -> Self {
        #[cfg(feature = "perfetto")]
        if log_overrides_perfetto && self.input.log.is_some() {
            self.perfetto = None;
        }
        #[cfg(not(feature = "perfetto"))]
        let _ = log_overrides_perfetto;
        self
    }

    fn validate(&self) -> AppResult<()> {
        if self.input.log.is_some() == self.input.bin.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "exactly one of `--log` or `--bin` must be provided",
            )
            .into());
        }

        #[cfg(feature = "perfetto")]
        if self.perfetto.is_some() && self.input.bin.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "`--perfetto` requires `--bin`",
            )
            .into());
        }

        Ok(())
    }
}

#[cfg(feature = "perfetto")]
fn log_overrides_perfetto(matches: &clap::ArgMatches) -> bool {
    fn source_priority(matches: &clap::ArgMatches, field: &str) -> u8 {
        match matches.value_source(field) {
            Some(clap::parser::ValueSource::CommandLine) => 2,
            Some(clap::parser::ValueSource::EnvVariable) => 1,
            _ if figment::providers::Env::prefixed("GWR_")
                .iter()
                .any(|(key, _)| key == field) =>
            {
                1
            }
            _ => 0,
        }
    }

    source_priority(matches, "log") > source_priority(matches, "perfetto")
}

#[tokio::main]
async fn main() -> AppResult<()> {
    let matches = CliPartial::command().get_matches();
    #[cfg(feature = "perfetto")]
    let log_overrides_perfetto = log_overrides_perfetto(&matches);
    #[cfg(not(feature = "perfetto"))]
    let log_overrides_perfetto = false;
    let args = Cli::parse_all_sources_with_matches(&matches).normalize(log_overrides_perfetto);
    args.validate()?;

    #[cfg(feature = "perfetto")]
    if let Some(perfetto_trace_output) = args.perfetto {
        perfetto::generate_perfetto_trace(
            args.input.bin.unwrap().as_path(),
            perfetto_trace_output.as_path(),
        );
        exit(0);
    }

    let _http_server = if args.no_server.unwrap() {
        None
    } else {
        Some(spawn().await?)
    };

    // Create an application.
    let mut app = App::new(args.input.log, args.input.bin);

    // Initialize the terminal user interface.
    let backend = CrosstermBackend::new(io::stderr());
    let terminal = Terminal::new(backend)?;
    let events = EventHandler::new(100);
    let mut tui = Tui::new(terminal, events);
    tui.init()?;

    // Start the main loop.
    while app.running {
        // Render the user interface.
        tui.draw(&mut app)?;
        // Handle events.
        match tui.events.next()? {
            Event::Tick => app.tick(),
            Event::Key(key_event) => handle_key_events(key_event, &mut app)?,
            Event::Mouse(_) => {}
            Event::Resize(_, _) => {}
        }
    }

    // Exit the user interface.
    tui.exit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;
    use figment::Figment;
    use figment::providers::Serialized;

    use super::{Cli, CliPartial};

    #[test]
    fn configuration_input_survives_an_empty_command_line() {
        let file_config = Figment::from(Serialized::default("log", PathBuf::from("trace.log")));
        let config = Cli::figment_extract(file_config);
        let cli = CliPartial::try_parse_from(["gwr-spotter"]).unwrap();
        let config = Cli::partial_to_config(Cli::clap_merge(config, cli));

        config.validate().unwrap();

        assert_eq!(config.input.log, Some(PathBuf::from("trace.log")));
        assert_eq!(config.input.bin, None);
    }

    #[test]
    fn command_line_input_replaces_a_configured_alternative() {
        let file_config = Figment::from(Serialized::default("log", PathBuf::from("trace.log")));
        let config = Cli::figment_extract(file_config);
        let cli = CliPartial::try_parse_from(["gwr-spotter", "--bin", "trace.bin"]).unwrap();
        let config = Cli::partial_to_config(Cli::clap_merge(config, cli));

        config.validate().unwrap();

        assert_eq!(config.input.log, None);
        assert_eq!(config.input.bin, Some(PathBuf::from("trace.bin")));
    }

    #[test]
    fn higher_priority_provider_replaces_a_configured_alternative() {
        let file = Cli::figment_extract(Figment::from(Serialized::default(
            "log",
            PathBuf::from("trace.log"),
        )));
        let environment = Cli::figment_extract(Figment::from(Serialized::default(
            "bin",
            PathBuf::from("trace.bin"),
        )));
        let config = Cli::partial_to_config(Cli::__gwr_config_merge_nested(file, environment));

        config.validate().unwrap();
        assert_eq!(config.input.log, None);
        assert_eq!(config.input.bin, Some(PathBuf::from("trace.bin")));
    }

    #[cfg(feature = "perfetto")]
    #[test]
    fn command_line_log_clears_configured_perfetto_output() {
        let file_config =
            Figment::from(Serialized::default("bin", PathBuf::from("trace.bin"))).merge(
                Serialized::default("perfetto", PathBuf::from("trace.pftrace")),
            );
        let config = Cli::figment_extract(file_config);
        let cli = CliPartial::try_parse_from(["gwr-spotter", "--log", "trace.log"]).unwrap();
        let config = Cli::partial_to_config(Cli::clap_merge(config, cli)).normalize(true);

        config.validate().unwrap();
        assert_eq!(config.input.log, Some(PathBuf::from("trace.log")));
        assert_eq!(config.input.bin, None);
        assert_eq!(config.perfetto, None);
    }

    #[cfg(feature = "perfetto")]
    #[test]
    fn rejects_log_and_perfetto_from_the_same_command_line() {
        let cli = CliPartial::try_parse_from([
            "gwr-spotter",
            "--log",
            "trace.log",
            "--perfetto",
            "trace.pftrace",
        ])
        .unwrap();
        let config =
            Cli::partial_to_config(Cli::clap_merge(CliPartial::default(), cli)).normalize(false);

        assert!(config.validate().is_err());
    }
}
