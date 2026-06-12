// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Small row/column wrapper for dynamic-programming tables.
//!
//! The coverage diff aligner uses several 2D tables, but stores them in a
//! single flat vector for compact allocation and cache-friendly iteration.
//! `DpTable` keeps the row-major indexing rule in one place so callers can work
//! in terms of `row` and `column` instead of repeating index arithmetic.

pub(crate) struct DpTable<T> {
    values: Vec<T>,
    column_count: usize,
}

impl<T: Clone> DpTable<T> {
    pub(crate) fn new(row_count: usize, column_count: usize, initial_value: T) -> Self {
        Self {
            values: vec![initial_value; row_count * column_count],
            column_count,
        }
    }
}

impl<T: Copy> DpTable<T> {
    pub(crate) fn get(&self, row: usize, column: usize) -> T {
        self.values[self.index(row, column)]
    }

    pub(crate) fn set(&mut self, row: usize, column: usize, value: T) {
        let index = self.index(row, column);
        self.values[index] = value;
    }

    fn index(&self, row: usize, column: usize) -> usize {
        row * self.column_count + column
    }
}
