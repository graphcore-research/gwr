// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::io;

use clap::Parser;
use steam_spotter::app::{App, AppResult};
use steam_spotter::event::{Event, EventHandler};
use steam_spotter::handler::handle_key_events;
use steam_spotter::tui::Tui;
use tui::{Terminal, backend::CrosstermBackend};

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "STEAM log/binary trace viewer")]
#[group(required = true, multiple = false)]
struct Cli {
    /// Provide a textual log file to be parsed
    #[arg(long)]
    log: Option<String>,

    /// Provide a capnp-based binary trace
    #[arg(long)]
    bin: Option<String>,
}

fn main() -> AppResult<()> {
    let args = Cli::parse();

    // Create an application.
    let mut app = App::new(args.log, args.bin);

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
