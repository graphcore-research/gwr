// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::fmt::Display;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub struct AppString {
    /// Current value
    value: String,

    /// Position of cursor in the editor area
    character_index: usize,
}

impl AppString {
    #[must_use]
    pub fn new(initial: &str) -> Self {
        Self {
            value: initial.to_string(),
            character_index: 0,
        }
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn set_value(&mut self, value: &str) {
        self.value = value.to_string();
        self.character_index = 0;
    }

    #[must_use]
    pub fn character_index(&self) -> usize {
        self.character_index
    }

    /// Move the cursor to the start of the string
    pub fn move_cursor_start(&mut self) {
        self.character_index = 0;
    }

    /// Move the cursor to the specified index
    pub fn move_cursor_to(&mut self, index: usize) {
        self.character_index = self.clamp_index(index);
    }

    /// Move the cursor to the end of the string
    pub fn move_cursor_end(&mut self) {
        self.character_index = self.value.len();
    }

    /// Move the cursor one character left
    pub fn move_cursor_left(&mut self) {
        let cursor_moved_left = self.character_index.saturating_sub(1);
        self.character_index = self.clamp_index(cursor_moved_left);
    }

    /// Move the cursor one character right
    pub fn move_cursor_right(&mut self) {
        let cursor_moved_right = self.character_index.saturating_add(1);
        self.character_index = self.clamp_index(cursor_moved_right);
    }

    /// Ensure the cursor index is valid
    fn clamp_index(&self, new_cursor_pos: usize) -> usize {
        new_cursor_pos.clamp(0, self.value.chars().count())
    }

    /// Insert a new character at the current character_index
    pub fn enter_char(&mut self, new_char: char) {
        let index = self.byte_index();
        self.value.insert(index, new_char);
        self.move_cursor_right();
    }

    /// Returns the byte index based on the character position.
    ///
    /// Since each character in a string can be contain multiple bytes, it's
    /// necessary to calculate the byte index based on the index of the
    /// character.
    fn byte_index(&self) -> usize {
        self.value
            .char_indices()
            .map(|(i, _)| i)
            .nth(self.character_index)
            .unwrap_or(self.value.len())
    }

    /// Remove the character to the left of the current index
    pub fn delete_char(&mut self) {
        // Method "remove" is not used because on String it works on bytes instead of
        // the chars.
        let current_index = self.character_index;
        if current_index > 0 {
            let from_left_to_current_index = current_index - 1;
            let before_char_to_delete = self.value.chars().take(from_left_to_current_index);
            let after_char_to_delete = self.value.chars().skip(current_index);
            self.value = before_char_to_delete.chain(after_char_to_delete).collect();
            self.move_cursor_left();
        }
    }

    /// Reset the character index to 0
    pub fn reset_cursor(&mut self) {
        self.character_index = 0;
    }

    /// Clear out the current string and reset the index
    pub fn clear_value(&mut self) {
        self.value.clear();
        self.reset_cursor();
    }

    pub fn handle_key_event(&mut self, key_event: KeyEvent) {
        if key_event.modifiers == KeyModifiers::CONTROL {
            match key_event.code {
                KeyCode::Char('a') => {
                    self.move_cursor_start();
                }
                KeyCode::Char('e') => {
                    self.move_cursor_end();
                }
                KeyCode::Char('u') => {
                    self.clear_value();
                }
                _ => {}
            }
        } else {
            match key_event.code {
                KeyCode::Char(to_insert) => self.enter_char(to_insert),
                KeyCode::Backspace => self.delete_char(),
                KeyCode::Delete => {
                    // Delete to the right (only if there are characters there)
                    if self.character_index() < self.value().len() {
                        self.move_cursor_right();
                        self.delete_char();
                    }
                }
                KeyCode::Left => self.move_cursor_left(),
                KeyCode::Right => self.move_cursor_right(),
                KeyCode::KeypadBegin => self.move_cursor_start(),
                KeyCode::End => self.move_cursor_end(),
                _ => {}
            }
        }
    }
}

impl Display for AppString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}
