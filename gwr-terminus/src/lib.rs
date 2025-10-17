// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use log::{error, info};
use ratatui::style::{Color, Style};

/// Trait defining an error handler function
pub trait Logger {
    fn error(&mut self, message: &str);
    fn info(&mut self, message: &str);
}

pub struct CliLogger;

impl Logger for CliLogger {
    fn error(&mut self, message: &str) {
        error!("{message}");
    }
    fn info(&mut self, message: &str) {
        info!("{message}");
    }
}

/// Trait for handling selection updates
///
/// Must also implement the [Logger] trait as some functions may
/// report errors.
pub trait UpdateSelect: Logger {
    fn update_select(&mut self, mode: char, index: usize);
}

pub trait Draw {
    fn draw(&self, frame: &mut ratatui::Frame);
}

pub mod app_string;
pub mod command;
pub mod recipe;
pub mod runner;
pub mod tui;
pub mod vec_with_index;
pub mod writer;

fn block_style(current_block: bool) -> Style {
    if current_block {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    }
}
