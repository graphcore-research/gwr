// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::error;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::filter::{Filter, start_background_filter};
use crate::renderer::Renderer;
use crate::rocket::SHARED_STATE;
use crate::{bin_loader, log_parser};

#[cfg(test)]
mod tests;

/// Size of blocks of data to read from the file and filter at a time
pub const CHUNK_SIZE: usize = 50000;

/// Size to start Vec with to prevent continually resizing
pub const INITIAL_SIZE: usize = CHUNK_SIZE;

pub trait ToTime {
    fn time_ns(&self) -> u128;
}

pub trait ToFullness {
    fn fullness(&self) -> u64;
}

#[derive(Debug, Clone)]
pub enum EventLine {
    Create {
        // Only need to keep the ID to be rendered.
        id: u64,
        time: Duration,
    },
    Connect {
        from_id: u64,
        to_id: u64,
        time: Duration,
    },
    Log {
        level: log::Level,
        id: u64,
        msg: String,
        time: Duration,
    },
    Enter {
        id: u64,
        entered: u64,
        fullness: u64,
        time: Duration,
    },
    Exit {
        id: u64,
        exited: u64,
        fullness: u64,
        time: Duration,
    },
    Value {
        id: u64,
        value: f64,
        time: Duration,
    },
}

impl ToTime for EventLine {
    fn time_ns(&self) -> u128 {
        match self {
            EventLine::Create { time, .. } => time.as_nanos(),
            EventLine::Connect { time, .. } => time.as_nanos(),
            EventLine::Enter { time, .. } => time.as_nanos(),
            EventLine::Exit { time, .. } => time.as_nanos(),
            EventLine::Value { time, .. } => time.as_nanos(),
            EventLine::Log { time, .. } => time.as_nanos(),
        }
    }
}

impl ToFullness for EventLine {
    fn fullness(&self) -> u64 {
        match self {
            EventLine::Create { .. } => 0,
            EventLine::Connect { .. } => 0,
            EventLine::Enter { fullness, .. } => *fullness,
            EventLine::Exit { fullness, .. } => *fullness,
            EventLine::Value { .. } => 0,
            EventLine::Log { .. } => 0,
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
    trace_absolute_index: usize,
    last_renderer_absolute_index: Option<usize>,
    fullness_absolute_index: Option<usize>,
    fullness_by_id: HashMap<u64, u64>,
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
            trace_absolute_index: 0,
            last_renderer_absolute_index: None,
            fullness_absolute_index: None,
            fullness_by_id: HashMap::new(),
        }
    }

    /// Handles the tick event of the terminal.
    pub fn tick(&mut self) {
        if let Some(s) = SHARED_STATE.lock().unwrap().command.take() {
            self.filter.lock().unwrap().set(&s);

            // Move to the top. Otherwise the current line index is often after
            // the matching lines.
            self.move_top();
            self.note_renderer_position_without_moving_trace();
        }

        if let Some(line_number) = SHARED_STATE.lock().unwrap().seek_line.take() {
            self.move_to_absolute_line(line_number);
        }

        self.publish_position();
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

    pub fn move_to_line(&mut self, line_number: usize) {
        let mut guard = self.renderer.lock().unwrap();
        guard.move_to_index(line_number.saturating_sub(1));
    }

    pub fn move_to_absolute_line(&mut self, line_number: usize) {
        self.trace_absolute_index = line_number.saturating_sub(1);
        let mut guard = self.renderer.lock().unwrap();
        guard.move_to_absolute_index(line_number.saturating_sub(1));
        self.last_renderer_absolute_index = Some(guard.current_absolute_index());
    }

    fn note_renderer_position_without_moving_trace(&mut self) {
        let renderer = self.renderer.lock().unwrap();
        self.last_renderer_absolute_index = Some(renderer.current_absolute_index());
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
            self.move_to_line(line_number);
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
            guard.move_to_index(line_number.saturating_sub(1));
        }
        self.numbers.clear();
    }

    fn publish_position(&mut self) {
        let renderer = self.renderer.lock().unwrap();
        let renderer_absolute_index = renderer.current_absolute_index();
        if self.last_renderer_absolute_index.is_none()
            || self.last_renderer_absolute_index != Some(renderer_absolute_index)
        {
            self.trace_absolute_index = renderer_absolute_index;
            self.last_renderer_absolute_index = Some(renderer_absolute_index);
        }

        if renderer.num_lines > 0 {
            self.trace_absolute_index = self.trace_absolute_index.min(renderer.num_lines - 1);
        } else {
            self.trace_absolute_index = 0;
        }

        if self.fullness_absolute_index != Some(self.trace_absolute_index) {
            self.fullness_by_id = renderer.fullnesses_at(self.trace_absolute_index);
            self.fullness_absolute_index = Some(self.trace_absolute_index);
        }

        let mut shared_state = SHARED_STATE.lock().unwrap();
        shared_state.current_line = if renderer.num_lines == 0 {
            0
        } else {
            self.trace_absolute_index + 1
        };
        shared_state.num_lines = renderer.num_lines;
        shared_state.current_time_ns = renderer.line_time(self.trace_absolute_index);
        shared_state.fullnesses = self
            .fullness_by_id
            .iter()
            .map(|(id, fullness)| format!("{id}={fullness}"))
            .collect();
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

    pub fn toggle_print_objects(&mut self) {
        let mut guard = self.renderer.lock().unwrap();
        guard.print_objects = !guard.print_objects;
    }

    pub fn toggle_print_details(&mut self) {
        let mut guard = self.renderer.lock().unwrap();
        guard.print_details = !guard.print_details;
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
