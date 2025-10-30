// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use log::{error, info};
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::widgets::{Block, Clear};

use crate::app_string::AppString;
use crate::command::{Command, process_select_command};
use crate::recipe::Recipe;
use crate::vec_with_index::VecWithIndex;
use crate::writer::ui::{render_app_string, render_help_line, render_history, render_message_area};
use crate::{Draw, Logger, UpdateSelect};

/// App holds the state of the application
pub struct App {
    /// Current value of the command box
    select_command: AppString,

    /// Filename to save recipe to
    recipe_filename: AppString,

    /// Recipe description
    recipe_description: AppString,

    /// Current application state which controls behaviour
    app_state: AppState,

    /// Previous application state
    previous_app_state: AppState,

    /// History of commands
    command_history: VecWithIndex<Command>,

    /// Whether or not the application is still running or should quit
    running: bool,

    /// Messages for user (e.g. errors)
    message: String,

    /// Whether the app is running within a TUI
    tui_mode: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum AppState {
    EditingSelectCommand,
    HistorySelection,
    HistoryEditing,
    ShowHelp,
    SaveFileName,
    SaveDescription,
}

#[must_use]
fn read_history(history_path: &PathBuf, error_handler: &mut impl Logger) -> Vec<String> {
    let mut lines = Vec::new();

    let file = match File::open(history_path) {
        Ok(file) => file,
        Err(e) => {
            error_handler.error(&format!("Failed to open {}: {e}", history_path.display()));
            return lines;
        }
    };

    let reader = BufReader::new(file);
    for line in reader.lines() {
        match line {
            Ok(line) => lines.push(line),
            Err(err) => error_handler.error(&format!("IO error: {err}")),
        }
    }
    lines
}

impl App {
    #[must_use]
    pub fn new(history_path: &PathBuf, recipes_folder: &str, tui_mode: bool) -> Self {
        // Setup the default filename to save recipe to as the RECIPES_FOLDER/.yaml
        let initial_recipe_name = Path::new(recipes_folder).join(".yaml");
        let initial_recipe_str = initial_recipe_name.as_os_str().to_str().unwrap();

        let mut app = Self {
            select_command: AppString::new(""),
            recipe_filename: AppString::new(initial_recipe_str),
            recipe_description: AppString::new(""),
            app_state: AppState::HistorySelection,
            previous_app_state: AppState::EditingSelectCommand,
            command_history: VecWithIndex::default(),
            running: true,
            message: String::new(),
            tui_mode,
        };

        // Move the filename cursor to just before the .yaml
        let initial_index = recipes_folder.len() + 1;
        app.recipe_filename.move_cursor_to(initial_index);

        // Build up the history by building a Command for each line of the history
        let history = read_history(history_path, &mut app);
        let command_history = app.command_history.rows_mut();
        for cmd_str in &history {
            command_history.push(Command::new(cmd_str));
        }

        // Move to the last line of the history as that is where the user normally wants
        // to start
        let history_len = app.command_history.rows().len();
        if history_len > 0 {
            app.command_history.set_index(history_len - 1);
        }
        app
    }

    #[must_use]
    pub fn select_command(&mut self) -> &mut AppString {
        &mut self.select_command
    }

    #[must_use]
    pub fn recipe_filename(&mut self) -> &mut AppString {
        &mut self.recipe_filename
    }

    #[must_use]
    pub fn recipe_description(&mut self) -> &mut AppString {
        &mut self.recipe_description
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn exit(&mut self) {
        self.running = false;
    }

    pub fn write_recipe(&mut self) {
        // TODO - prompt if file found
        let filename = self.recipe_filename.value().to_string();
        let file_path = Path::new(filename.as_str());

        let recipe = Recipe::new(self.recipe_description.value(), self.command_history.rows());
        let yaml = serde_yaml_ng::to_string(&recipe).unwrap();
        if fs::write(file_path, yaml).is_err() {
            self.error(&format!("Failed to write to `{}`", file_path.display()));
        } else {
            self.info(&format!("`{}` written", file_path.display()));
        }
    }

    #[must_use]
    pub fn app_state(&self) -> AppState {
        self.app_state
    }

    #[must_use]
    pub fn get_current_history_string(&mut self) -> &mut AppString {
        self.command_history.selected_mut().app_string_mut()
    }

    #[must_use]
    pub fn command_history(&mut self) -> &mut VecWithIndex<Command> {
        &mut self.command_history
    }

    pub fn set_app_state(&mut self, app_state: AppState) {
        if self.app_state != AppState::SaveDescription && self.app_state != AppState::SaveFileName {
            // Don't remember one of the Save modes
            self.previous_app_state = self.app_state;
        }
        self.app_state = app_state;
    }

    pub fn restore_app_state(&mut self) {
        self.app_state = self.previous_app_state;
    }

    pub fn toggle_current_row_selected(&mut self) {
        self.command_history.selected_mut().toggle_select();
    }

    pub fn process_select_command(&mut self) {
        let max_index = self.command_history.rows().len();
        let command = self.select_command.value().to_string();
        process_select_command(self, &command, max_index);
    }
}

impl Draw for App {
    fn draw(&self, frame: &mut Frame) {
        if self.app_state == AppState::ShowHelp {
            crate::writer::ui::render_help(frame, frame.area());
            return;
        }

        let vertical = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(5),
        ]);
        let [help_area, select_command_area, history_area, messages_area] =
            vertical.areas(frame.area());

        render_help_line(frame, help_area, &self.app_state);
        render_app_string(
            frame,
            select_command_area,
            "Selection Command",
            &self.select_command,
            self.app_state == AppState::EditingSelectCommand,
        );
        render_history(frame, history_area, &self.command_history, &self.app_state);
        render_message_area(frame, messages_area, &self.message);

        if self.app_state == AppState::SaveDescription || self.app_state == AppState::SaveFileName {
            let block = Block::bordered().title("Save Recipe");
            let area = popup_area(history_area, 80, 8);
            // Clear out the background
            frame.render_widget(Clear, area);

            let vertical = Layout::vertical([Constraint::Length(3), Constraint::Length(3)]);

            let inner = block.inner(area);
            let [file_name_area, description_area] = vertical.areas(inner);
            frame.render_widget(block, area);
            render_app_string(
                frame,
                file_name_area,
                "Recipe Filename",
                &self.recipe_filename,
                self.app_state == AppState::SaveFileName,
            );
            render_app_string(
                frame,
                description_area,
                "Recipe Description",
                &self.recipe_description,
                self.app_state == AppState::SaveDescription,
            );
        }
    }
}

fn popup_area(area: Rect, percent_x: u16, lines_y: u16) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(lines_y)]).flex(Flex::Center);
    let horizontal = Layout::horizontal([Constraint::Percentage(percent_x)]).flex(Flex::Center);
    let [area] = vertical.areas(area);
    let [area] = horizontal.areas(area);
    area
}

impl UpdateSelect for App {
    fn update_select(&mut self, mode: char, index: usize) {
        if index < self.command_history.rows().len() {
            match mode {
                's' => self.command_history.rows_mut()[index].select(),
                'd' => self.command_history.rows_mut()[index].deselect(),
                't' => self.command_history.rows_mut()[index].toggle_select(),
                _ => self.error("Invalid mode. Use 's'elect, 'd'eselect, 't'oggle"),
            }
        } else {
            self.error(&format!("Invalid index {index}"));
        }
    }
}

impl Logger for App {
    fn error(&mut self, message: &str) {
        if self.tui_mode {
            // When running in the TUI use the messages box
            self.message = message.to_string();
        } else {
            // Otherwise log to the command-line
            error!("{message}");
        }
    }

    fn info(&mut self, message: &str) {
        if self.tui_mode {
            // When running in the TUI use the messages box
            self.message = message.to_string();
        } else {
            // Otherwise log to the command-line
            info!("{message}");
        }
    }
}
