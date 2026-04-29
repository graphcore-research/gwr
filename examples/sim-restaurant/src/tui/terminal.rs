// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::{io, panic};

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::Backend;

use crate::tui::event::AppResult;

#[derive(Debug)]
pub struct Tui<B: Backend> {
    terminal: Terminal<B>,
}

impl<B: Backend> Tui<B> {
    pub fn new(terminal: Terminal<B>) -> Self {
        Self { terminal }
    }

    pub fn init(&mut self) -> AppResult<()>
    where
        <B as Backend>::Error: 'static,
    {
        terminal::enable_raw_mode()?;
        crossterm::execute!(io::stderr(), EnterAlternateScreen, EnableMouseCapture)?;

        let panic_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic| {
            Self::reset().expect("failed to reset the terminal");
            panic_hook(panic);
        }));

        self.terminal.hide_cursor()?;
        self.terminal.clear()?;
        Ok(())
    }

    pub fn draw(&mut self, app: &mut crate::tui::app::App) -> AppResult<()>
    where
        <B as Backend>::Error: 'static,
    {
        self.terminal
            .draw(|frame| crate::tui::ui::render(app, frame))?;
        Ok(())
    }

    fn reset() -> AppResult<()> {
        terminal::disable_raw_mode()?;
        crossterm::execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
        Ok(())
    }

    pub fn exit(&mut self) -> AppResult<()>
    where
        <B as Backend>::Error: 'static,
    {
        Self::reset()?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}
