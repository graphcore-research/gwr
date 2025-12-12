// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::error;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use crate::filter::{Filter, start_background_filter};
use crate::renderer::Renderer;
use crate::rocket::SHARED_STATE;
use crate::{bin_loader, log_parser};

mod tests;

/// Size of blocks of data to read from the file and filter at a time
pub const CHUNK_SIZE: usize = 50000;

/// Size to start Vec with to prevent continually resizing
pub const INITIAL_SIZE: usize = CHUNK_SIZE;

pub trait ToTime {
    fn time(&self) -> f64;
}

pub trait ToFullness {
    fn fullness(&self) -> u64;
}

#[derive(Debug, Clone)]
pub enum EventLine {
    Create {
        // Only need to keep the ID to be rendered.
        id: u64,
        time: f64,
    },
    Connect {
        from_id: u64,
        to_id: u64,
        time: f64,
    },
    Log {
        level: log::Level,
        id: u64,
        msg: String,
        time: f64,
    },
    Enter {
        id: u64,
        entered: u64,
        fullness: u64,
        time: f64,
    },
    Exit {
        id: u64,
        exited: u64,
        fullness: u64,
        time: f64,
    },
    Value {
        id: u64,
        value: f64,
        time: f64,
    },
}

impl ToTime for EventLine {
    fn time(&self) -> f64 {
        match self {
            EventLine::Create { id: _, time } => *time,
            EventLine::Connect {
                from_id: _,
                to_id: _,
                time,
            } => *time,
            EventLine::Enter {
                id: _,
                entered: _,
                fullness: _,
                time,
            } => *time,
            EventLine::Exit {
                id: _,
                exited: _,
                fullness: _,
                time,
            } => *time,
            EventLine::Value {
                id: _,
                value: _,
                time,
            } => *time,
            EventLine::Log {
                level: _,
                id: _,
                msg: _,
                time,
            } => *time,
        }
    }
}

impl ToFullness for EventLine {
    fn fullness(&self) -> u64 {
        match self {
            EventLine::Create { id: _, time: _ } => 0,
            EventLine::Connect {
                from_id: _,
                to_id: _,
                time: _,
            } => 0,
            EventLine::Enter {
                id: _,
                entered: _,
                fullness,
                time: _,
            } => *fullness,
            EventLine::Exit {
                id: _,
                exited: _,
                fullness,
                time: _,
            } => *fullness,
            EventLine::Value {
                id: _,
                value: _,
                time: _,
            } => 0,
            EventLine::Log {
                level: _,
                id: _,
                msg: _,
                time: _,
            } => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputState {
    Default,
    Goto,
    Help,
    Numbers,
    Search,
}

/// Application result type.
pub type AppResult<T> = std::result::Result<T, Box<dyn error::Error>>;

/// Application.
pub struct App {
    /// Is the application running?
    pub running: bool,
    pub renderer: Arc<Mutex<Renderer>>,
    pub filter: Arc<Mutex<Filter>>,
    pub input_state: InputState,
    pub numbers: String,
}

impl App {
    /// Constructs a new instance of [`App`].
    #[must_use]
    pub fn new(log_file_path: Option<PathBuf>, bin_file_path: Option<PathBuf>) -> Self {
        let (tx, rx) = channel();
        let renderer = Arc::new(Mutex::new(Renderer::new()));
        let filter = Arc::new(Mutex::new(Filter::new(tx)));

        if let Some(log_file_path) = log_file_path {
            log_parser::start_background_load(
                log_file_path.as_path(),
                renderer.clone(),
                filter.clone(),
            );
        } else {
            bin_loader::start_background_load(
                bin_file_path.unwrap().as_path(),
                renderer.clone(),
                filter.clone(),
            );
        }
        start_background_filter(rx, renderer.clone(), filter.clone());

        Self {
            running: true,
            renderer,
            filter,
            input_state: InputState::Default,
            numbers: String::new(),
        }
    }

    /// Handles the tick event of the terminal.
    pub fn tick(&mut self) {
        if let Some(s) = SHARED_STATE.lock().unwrap().command.take() {
            self.filter.lock().unwrap().set(&s);

            // Move to the top. Otherwise the current line index is often after
            // the matching lines.
            self.move_top();
        }
    }

    /// Set running to false to quit the application.
    pub fn quit(&mut self) {
        self.running = false;
    }

    pub fn move_top(&mut self) {
        let mut guard = self.renderer.lock().unwrap();
        guard.move_top();
    }

    pub fn move_bottom(&mut self) {
        let mut guard = self.renderer.lock().unwrap();
        guard.move_bottom();
    }

    pub fn move_down_lines(&mut self, num_lines: usize) {
        let mut guard = self.renderer.lock().unwrap();
        guard.move_down_lines(num_lines);
    }

    pub fn move_up_lines(&mut self, num_lines: usize) {
        let mut guard = self.renderer.lock().unwrap();
        guard.move_up_lines(num_lines);
    }

    pub fn move_down_block(&mut self) {
        let mut guard = self.renderer.lock().unwrap();
        let num_lines = guard.block_move_lines;
        guard.move_down_lines(num_lines);
    }

    pub fn move_up_block(&mut self) {
        let mut guard = self.renderer.lock().unwrap();
        let num_lines = guard.block_move_lines;
        guard.move_up_lines(num_lines);
    }

    pub fn move_to_number(&mut self) {
        if let Ok(line_number) = self.numbers.parse::<usize>() {
            let mut guard = self.renderer.lock().unwrap();
            // Position is 0-based while line numbers are not (hence -1).
            guard.move_to_index(line_number - 1);
        }
        self.numbers.clear();
    }

    pub fn move_down_n(&mut self) {
        if let Ok(num_lines) = self.numbers.parse() {
            let mut guard = self.renderer.lock().unwrap();
            guard.move_down_lines(num_lines);
        }
        self.numbers.clear();
    }

    pub fn move_to_percent(&mut self) {
        if let Ok(percent) = self.numbers.parse::<usize>() {
            let mut guard = self.renderer.lock().unwrap();
            let line_number = guard.num_render_lines * percent / 100;
            guard.move_to_index(line_number - 1);
        }
        self.numbers.clear();
    }

    #[must_use]
    pub fn state(&self) -> InputState {
        self.input_state
    }

    pub fn set_state(&mut self, new_state: InputState) {
        self.input_state = new_state;
    }

    pub fn toggle_plot_fullness(&mut self) {
        let mut guard: std::sync::MutexGuard<'_, Renderer> = self.renderer.lock().unwrap();
        guard.plot_fullness = !guard.plot_fullness;
    }

    pub fn toggle_print_names(&mut self) {
        let mut guard = self.renderer.lock().unwrap();
        guard.print_names = !guard.print_names;
    }

    pub fn toggle_print_packets(&mut self) {
        let mut guard = self.renderer.lock().unwrap();
        guard.print_packets = !guard.print_packets;
    }

    pub fn toggle_print_times(&mut self) {
        let mut guard = self.renderer.lock().unwrap();
        guard.print_times = !guard.print_times;
    }

    pub fn set_frame_size(&mut self, frame_height: usize) {
        let mut guard = self.renderer.lock().unwrap();
        guard.set_frame_size(frame_height);
    }

    pub fn push_number_char(&mut self, c: char) {
        self.numbers.push(c);
    }

    pub fn pop_number_char(&mut self) {
        self.numbers.pop();
    }

    pub fn clear_numbers(&mut self) {
        self.numbers.clear();
    }
}
