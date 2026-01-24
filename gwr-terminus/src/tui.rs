// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::{io, panic};

use color_eyre::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::Backend;

use crate::Draw;

/// Representation of a terminal user interface.
///
/// It is responsible for setting up the terminal,
/// initializing the interface and handling the draw events.
#[derive(Debug)]
pub struct Tui<B: Backend> {
    /// Interface to the Terminal.
    terminal: Terminal<B>,
}

impl<B: Backend> Tui<B> {
    /// Constructs a new instance of [`Tui`].
    pub fn new(terminal: Terminal<B>) -> Self {
        Self { terminal }
    }

    /// Initializes the terminal interface.
    ///
    /// It enables the raw mode and sets terminal properties.
    pub fn init(&mut self) -> Result<()>
    where
        <B as Backend>::Error: Send + Sync,
        <B as Backend>::Error: 'static,
    {
        terminal::enable_raw_mode()?;
        crossterm::execute!(io::stderr(), EnterAlternateScreen, EnableMouseCapture)?;

        // Define a custom panic hook to reset the terminal properties.
        // This way, you won't have your terminal messed up if an unexpected error
        // happens.
        let panic_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic| {
            Self::reset().expect("failed to reset the terminal");
            panic_hook(panic);
        }));

        self.terminal.hide_cursor()?;
        self.terminal.clear()?;
        Ok(())
    }

    /// [`Draw`] the terminal interface by rendering the widgets.
    ///
    /// [`Draw`]: ratatui::Terminal::draw
    pub fn draw(&mut self, app: &mut impl Draw) -> Result<()>
    where
        <B as Backend>::Error: Send + Sync,
        <B as Backend>::Error: 'static,
    {
        // self.terminal.draw(|frame| ui::render(app, frame))?;
        self.terminal.draw(|frame| app.draw(frame))?;
        Ok(())
    }

    /// Resets the terminal interface.
    ///
    /// This function is also used for the panic hook to revert
    /// the terminal properties if unexpected errors occur.
    fn reset() -> Result<()> {
        terminal::disable_raw_mode()?;
        crossterm::execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
        Ok(())
    }

    /// Exits the terminal interface.
    ///
    /// It disables the raw mode and reverts back the terminal properties.
    pub fn exit(&mut self) -> Result<()>
    where
        <B as Backend>::Error: Send + Sync,
        <B as Backend>::Error: 'static,
    {
        Self::reset()?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}
