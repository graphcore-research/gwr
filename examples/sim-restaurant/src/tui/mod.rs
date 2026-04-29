// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

pub mod app;
pub mod event;
pub mod terminal;
pub mod ui;

use std::io;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::recording::RecordedSimulation;
use crate::tui::event::{AppResult, Event, EventHandler};
use crate::tui::terminal::Tui;

pub fn run(recording: RecordedSimulation) -> AppResult<()> {
    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;
    let events = EventHandler::new(100);
    let mut tui = Tui::new(terminal);
    let mut app = app::App::new(recording);

    tui.init()?;

    loop {
        tui.draw(&mut app)?;
        match events.next()? {
            Event::Tick => app.handle_tick(),
            Event::Key(key) => {
                if app.handle_key(key) {
                    break;
                }
            }
            Event::Resize(_, _) => {}
        }
    }

    tui.exit()?;
    Ok(())
}
