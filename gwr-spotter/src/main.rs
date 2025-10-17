// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::path::PathBuf;
#[cfg(feature = "perfetto")]
use std::process::exit;
use std::{io, thread};

use clap::{Args, Parser};
use gwr_spotter::app::{App, AppResult};
use gwr_spotter::event::{Event, EventHandler};
use gwr_spotter::handler::handle_key_events;
#[cfg(feature = "perfetto")]
use gwr_spotter::perfetto;
use gwr_spotter::rocket::rocket;
use gwr_spotter::tui::Tui;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::runtime::Runtime;

/// Input subcommand arguments.
#[derive(Args)]
#[group(required = true, multiple = false)]
struct InputOptions {
    /// Provide a textual log file to be parsed
    #[arg(long)]
    log: Option<PathBuf>,

    /// Provide a capnp-based binary trace
    #[arg(long, group = "perfetto_compat")]
    bin: Option<PathBuf>,
}

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "GWR log/binary trace viewer")]
struct Cli {
    #[command(flatten)]
    input: InputOptions,

    #[cfg(feature = "perfetto")]
    /// Generate Perfetto output from GWR binary trace with this name
    ///
    /// gwr-spotter will exit having produced the Perfetto trace.
    #[arg(long, requires = "perfetto_compat")]
    perfetto: Option<PathBuf>,
}

fn spawn_rocket() {
    // Create new thread
    thread::spawn(|| {
        // Create new Tokio runtime
        let rt = Runtime::new().unwrap();

        // Create async function
        rt.block_on(async {
            let _start = rocket().launch().await;
        });
    });
}

#[rocket::main]
async fn main() -> AppResult<()> {
    let args = Cli::parse();

    #[cfg(feature = "perfetto")]
    if let Some(perfetto_trace_output) = args.perfetto {
        perfetto::generate_perfetto_trace(
            args.input.bin.unwrap().as_path(),
            perfetto_trace_output.as_path(),
        );
        exit(0);
    }

    spawn_rocket();

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
