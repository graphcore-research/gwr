// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::fmt::Display;

use crate::app_string::AppString;
use crate::{Logger, UpdateSelect};

/// Command holds the state related to a single command
pub struct Command {
    /// Command string
    command: AppString,

    /// Whether or not this command is currently selected
    selected: bool,
}

impl Command {
    #[must_use]
    pub fn new(command: &str) -> Self {
        let command = remove_history_index(command);
        Self {
            command: AppString::new(command.as_str()),
            selected: false,
        }
    }

    #[must_use]
    pub fn app_string_mut(&mut self) -> &mut AppString {
        &mut self.command
    }

    #[must_use]
    pub fn app_string(&self) -> &AppString {
        &self.command
    }

    #[must_use]
    pub fn selected(&self) -> bool {
        self.selected
    }

    pub fn select(&mut self) {
        self.selected = true;
    }

    pub fn deselect(&mut self) {
        self.selected = false;
    }

    pub fn toggle_select(&mut self) {
        self.selected = !self.selected;
    }

    #[must_use]
    pub fn command(&self) -> &str {
        self.command.value()
    }
}

impl Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.command.value())
    }
}

/// Take a line from a history file like:
///     996  cd sandboxes
/// and remove the index at the start.
fn remove_history_index(command: &str) -> String {
    let command = command.trim().to_string();
    if command.find(char::is_whitespace).is_none() {
        return command.trim().to_string();
    }

    let (index_str, command_rest) = command.split_once(char::is_whitespace).unwrap();
    let index_str = index_str.trim();
    match index_str.parse::<i32>() {
        Ok(_index) => command_rest.trim().to_string(),
        Err(_e) => command.trim().to_string(),
    }
}

pub fn process_select_command(updater: &mut impl UpdateSelect, command: &str, max_index: usize) {
    if command.is_empty() {
        return;
    }

    let select_command = command.to_string();
    let blocks: Vec<&str> = select_command.split(';').collect();
    for block in blocks {
        let mode = match block.chars().next() {
            Some(c) => c,
            None => continue,
        };

        // Skip over mode character
        let mut rest = block.chars();
        rest.next();

        let ranges: Vec<&str> = rest.as_str().split(',').collect();
        for range in ranges {
            process_select_range(updater, mode, range, max_index);
        }
    }
}

fn process_select_range(
    updater: &mut impl UpdateSelect,
    mode: char,
    range: &str,
    max_index: usize,
) {
    if range.is_empty() {
        return;
    }

    let indices: Vec<&str> = range.split('-').collect();
    match indices.len() {
        1 => {
            if indices[0].eq("*") {
                for i in 0..max_index {
                    updater.update_select(mode, i);
                }
            } else if let Some(i) = parse_index(updater, indices[0]) {
                updater.update_select(mode, i);
            }
        }
        2 => {
            let begin = match parse_index(updater, indices[0]) {
                Some(index) => index,
                None => return,
            };
            let end = match parse_index(updater, indices[1]) {
                Some(index) => index,
                None => return,
            };

            for i in begin..end + 1 {
                updater.update_select(mode, i);
            }
        }
        _ => updater.error(&format!("Unable to parse {range}")),
    }
}

fn parse_index(error_handler: &mut impl Logger, index_str: &str) -> Option<usize> {
    match index_str.parse::<usize>() {
        Ok(index) => Some(index),
        Err(e) => {
            error_handler.error(&format!("Unable to parse '{index_str}'\n{e}"));
            None
        }
    }
}

#[cfg(test)]
struct TestUpdater {
    select: Vec<usize>,
    deselect: Vec<usize>,
    toggle: Vec<usize>,
    error: Vec<String>,
    info: Vec<String>,
}

#[cfg(test)]
impl Default for TestUpdater {
    fn default() -> Self {
        Self {
            select: Vec::new(),
            deselect: Vec::new(),
            toggle: Vec::new(),
            error: Vec::new(),
            info: Vec::new(),
        }
    }
}

#[cfg(test)]
impl UpdateSelect for TestUpdater {
    fn update_select(&mut self, mode: char, index: usize) {
        match mode {
            's' => self.select.push(index),
            'd' => self.deselect.push(index),
            't' => self.toggle.push(index),
            _ => self.error("Invalid mode. Use 's'elect, 'd'eselect, 't'oggle"),
        }
    }
}

#[cfg(test)]
impl Logger for TestUpdater {
    fn error(&mut self, message: &str) {
        self.error.push(message.to_string());
    }
    fn info(&mut self, message: &str) {
        self.info.push(message.to_string());
    }
}

#[test]
fn parse_error_command() {
    let max_index = 10;
    let mut updater = TestUpdater::default();
    process_select_command(&mut updater, "c1", max_index);
    assert_eq!(updater.select.len(), 0);
    assert_eq!(updater.deselect.len(), 0);
    assert_eq!(updater.toggle.len(), 0);
    assert_eq!(
        updater.error,
        ["Invalid mode. Use 's'elect, 'd'eselect, 't'oggle"]
    );
    assert_eq!(updater.info.len(), 0);
}

#[test]
fn parse_error_invalid_range() {
    let max_index = 10;
    let mut updater = TestUpdater::default();
    process_select_command(&mut updater, "t1-", max_index);
    assert_eq!(updater.select.len(), 0);
    assert_eq!(updater.deselect.len(), 0);
    assert_eq!(updater.toggle.len(), 0);
    assert_eq!(
        updater.error,
        ["Unable to parse ''\ncannot parse integer from empty string"]
    );
    assert_eq!(updater.info.len(), 0);
}

#[test]
fn test_parse() {
    let max_index = 10;
    let mut updater = TestUpdater::default();
    process_select_command(&mut updater, "s1", max_index);
    assert_eq!(updater.select, [1]);
    assert_eq!(updater.deselect.len(), 0);
    assert_eq!(updater.toggle.len(), 0);
    assert_eq!(updater.error.len(), 0);
    assert_eq!(updater.info.len(), 0);
}

#[test]
fn test_parse_range() {
    let max_index = 10;
    let mut updater = TestUpdater::default();
    process_select_command(&mut updater, "t5-10", max_index);
    assert_eq!(updater.select.len(), 0);
    assert_eq!(updater.deselect.len(), 0);
    assert_eq!(updater.toggle, [5, 6, 7, 8, 9, 10]);
    assert_eq!(updater.error.len(), 0);
    assert_eq!(updater.info.len(), 0);
}

#[test]
fn test_parse_multi_command() {
    let max_index = 5;
    let mut updater = TestUpdater::default();
    process_select_command(&mut updater, "s*;d2-3;t4,8", max_index);
    assert_eq!(updater.select, [0, 1, 2, 3, 4]);
    assert_eq!(updater.deselect, [2, 3]);
    assert_eq!(updater.toggle, [4, 8]);
    assert_eq!(updater.error.len(), 0);
    assert_eq!(updater.info.len(), 0);
}
