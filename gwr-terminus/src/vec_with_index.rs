// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub struct VecWithIndex<T> {
    /// Current row index
    index: usize,

    /// Rows that are managed by this index
    pub rows: Vec<T>,

    /// Keep index at the end
    keep_index_end: bool,
}

impl<T> VecWithIndex<T> {
    #[must_use]
    pub fn new(keep_index_end: bool) -> Self {
        Self {
            index: 0,
            rows: Vec::new(),
            keep_index_end,
        }
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.index = 0;
    }

    #[must_use]
    pub fn index(&self) -> usize {
        if self.keep_index_end && !self.rows.is_empty() {
            self.rows.len() - 1
        } else {
            self.index
        }
    }

    #[must_use]
    pub fn selected(&self) -> &T {
        &self.rows[self.index]
    }

    #[must_use]
    pub fn selected_mut(&mut self) -> &mut T {
        &mut self.rows[self.index]
    }

    #[must_use]
    pub fn rows(&self) -> &Vec<T> {
        &self.rows
    }

    #[must_use]
    pub fn rows_mut(&mut self) -> &mut Vec<T> {
        &mut self.rows
    }

    pub fn set_index(&mut self, new_index: usize) {
        self.index = self.clamp_history_row(new_index);

        // The user has moved the index, so don't auto-update any more
        self.keep_index_end = false;
    }

    pub fn move_row_up(&mut self, n: usize) {
        let index_up = self.index.saturating_sub(n);
        self.index = self.clamp_history_row(index_up);

        // The user has moved the index, so don't auto-update any more
        self.keep_index_end = false;
    }

    pub fn move_row_down(&mut self, n: usize) {
        let index_down = self.index.saturating_add(n);
        self.index = self.clamp_history_row(index_down);

        // The user has moved the index, so don't auto-update any more
        self.keep_index_end = false;
    }

    fn clamp_history_row(&self, new_index: usize) -> usize {
        if self.rows().is_empty() {
            0
        } else {
            new_index.clamp(0, self.rows.len() - 1)
        }
    }

    pub fn handle_key_event(&mut self, key_event: KeyEvent) {
        // Number of lines to move when pressing PageUp/Down
        let page_size = 10;

        if key_event.modifiers == KeyModifiers::CONTROL {
            match key_event.code {
                KeyCode::Char('f') => {
                    self.move_row_down(page_size);
                }
                KeyCode::Char('u') => {
                    self.move_row_up(page_size);
                }
                _ => {}
            }
        } else {
            match key_event.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.move_row_up(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.move_row_down(1);
                }
                KeyCode::PageUp => {
                    self.move_row_up(page_size);
                }
                KeyCode::PageDown => {
                    self.move_row_down(page_size);
                }
                _ => {}
            }
        }
    }
}

impl<T> Default for VecWithIndex<T> {
    fn default() -> Self {
        Self::new(false)
    }
}
