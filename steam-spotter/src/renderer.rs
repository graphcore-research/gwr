// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;

use crate::app::{CHUNK_SIZE, EventLine, INITIAL_SIZE, ToFullness, ToTime};

const UNKNOWN: &str = "???";

pub struct Renderer {
    tag_to_name: HashMap<u64, String>,

    /// Current location within the file
    render_index: usize,

    /// Vector of blocks of lines that can be taken out for processing
    blocks: Vec<Option<Vec<EventLine>>>,

    /// Total number of lines (sum of blocks)
    pub num_lines: usize,

    /// When filtering, which lines are to be shown
    pub render_indices: Option<Vec<usize>>,
    pub num_render_lines: usize,

    pub frame_height: usize,
    pub block_move_lines: usize,

    pub plot_fullness: bool,

    pub print_names: bool,
    pub print_packets: bool,
    pub print_times: bool,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            tag_to_name: HashMap::with_capacity(INITIAL_SIZE),
            blocks: Vec::with_capacity(INITIAL_SIZE),
            render_indices: None,
            num_render_lines: 0,

            num_lines: 0,
            render_index: 0,

            // Choose a sensible starting default will be updated on the fly
            frame_height: 25,
            block_move_lines: 25 / 3,

            plot_fullness: false,

            print_names: false,
            print_packets: false,
            print_times: true,
        }
    }

    /// Return the line number at which the current rendering starts
    pub fn current_render_line_number(&self) -> usize {
        self.render_index + 1
    }

    pub fn set_frame_size(&mut self, frame_height: usize) {
        self.frame_height = frame_height;
        self.block_move_lines = self.frame_height / 3;
    }

    pub fn take_chunk(&mut self, block_index: usize) -> Option<Vec<EventLine>> {
        match self.blocks.get_mut(block_index) {
            Some(optional_block) => optional_block.take(),
            None => None,
        }
    }

    pub fn restore_chunk(&mut self, block_index: usize, chunk: Option<Vec<EventLine>>) {
        self.blocks[block_index] = chunk;
    }

    fn name_tag<'a>(&'a self, tag: &u64, tmp: &'a mut String) -> &'a str {
        tmp.clear();
        tmp.push_str(tag.to_string().as_str());

        if self.print_names {
            tmp.push_str(": ");
            match tag {
                0 => {
                    tmp.push_str("root");
                }
                _ => match self.tag_to_name.get(tag) {
                    Some(name) => tmp.push_str(name),
                    None => tmp.push_str(UNKNOWN),
                },
            };
        }

        tmp.as_str()
    }

    fn packet_tag<'a>(&'a self, tag: &u64, tmp: &'a mut String) -> &'a str {
        tmp.clear();
        tmp.push_str(tag.to_string().as_str());

        if self.print_packets {
            tmp.push_str(": ");
            match tag {
                0 => tmp.push_str("root"),
                _ => match self.tag_to_name.get(tag) {
                    Some(name) => tmp.push_str(name),
                    None => tmp.push_str(UNKNOWN),
                },
            };
        }

        tmp.as_str()
    }

    pub fn line_from_index(&self, line_index: usize) -> Option<&EventLine> {
        let block_index = line_index / CHUNK_SIZE;
        let chunk = &self.blocks[block_index];
        if chunk.is_none() {
            return None;
        }
        let chunk = chunk.as_ref().unwrap();
        let chunk_offset = line_index % CHUNK_SIZE;
        chunk.get(chunk_offset)
    }

    pub fn line_time(&self, line_index: usize) -> f64 {
        if let Some(line) = self.line_from_index(line_index) {
            line.time()
        } else {
            0.0
        }
    }

    pub fn line_fullness(&self, line_index: usize) -> u64 {
        if let Some(line) = self.line_from_index(line_index) {
            line.fullness()
        } else {
            0
        }
    }

    pub fn render_line(&self, line_index: usize) -> String {
        let event_line = self.line_from_index(line_index);
        if event_line.is_none() {
            return "".to_owned();
        }

        let event_line = event_line.unwrap();
        let mut name_tmp = String::new();
        let mut pkt_tmp = String::new();
        let (mut line, time) = match event_line {
            EventLine::Enter {
                tag,
                entered,
                fullness,
                time,
            } => {
                let name = self.name_tag(tag, &mut name_tmp);
                let packet = self.packet_tag(entered, &mut pkt_tmp);
                (
                    format!("{}: <= {} ({})", name, packet, fullness).to_owned(),
                    time,
                )
            }

            EventLine::Exit {
                tag,
                exited,
                fullness,
                time,
            } => {
                let name = self.name_tag(tag, &mut name_tmp);
                let packet = self.packet_tag(exited, &mut pkt_tmp);
                (
                    format!("{}: => {} ({})", name, packet, fullness).to_owned(),
                    time,
                )
            }

            EventLine::Log {
                level: _,
                tag,
                msg,
                time,
            } => {
                let name = self.name_tag(tag, &mut name_tmp);
                (format!("{}: {}", name, msg).to_owned(), time)
            }

            EventLine::Create { tag, time } => {
                let name = self.name_tag(tag, &mut name_tmp);
                (format!("{}: created", name).to_owned(), time)
            }
        };

        if self.print_times {
            line.push_str(format!(" @{:.1}ns", time).as_str());
        }

        line
    }

    pub fn add_chunk(&mut self, lines: Vec<EventLine>) {
        self.num_lines += lines.len();
        self.blocks.push(Some(lines));
    }

    pub fn extend_tag_to_name(&mut self, tag_to_name: HashMap<u64, String>) {
        self.tag_to_name.extend(tag_to_name);
    }

    fn render_index_to_absolute_index(&self, line: usize) -> usize {
        if let Some(indices) = &self.render_indices {
            if let Some(index) = indices.get(line) {
                return *index;
            }
            if let Some(index) = indices.last() {
                return *index;
            }
        }
        0
    }

    fn absoulte_index_to_render_index(&self, index: usize) -> usize {
        if let Some(indices) = &self.render_indices {
            for (i, render_index) in indices.iter().enumerate() {
                if *render_index >= index {
                    return i;
                }
            }
            return indices.len();
        }
        0
    }

    /// Change the current indices for a new set of ones to render.
    ///
    /// Tries to maintain a fixed position within the file.
    pub fn set_render_indices(&mut self, indices: Vec<usize>) {
        let absolute_index = self.render_index_to_absolute_index(self.render_index);

        self.num_render_lines = indices.len();
        self.render_indices = Some(indices);

        self.render_index = self.absoulte_index_to_render_index(absolute_index);
    }

    /// Move to a line index.
    ///
    /// Indices start at 0, line numbers shown to the user start at 1.
    pub fn move_to_index(&mut self, index: usize) {
        if index > self.num_render_lines {
            self.render_index = self.num_render_lines - 1;
        } else {
            self.render_index = index;
        }
    }

    pub fn move_top(&mut self) {
        self.render_index = 0;
    }

    pub fn move_bottom(&mut self) {
        // Leave some space at the end of the file
        let gap = 5;
        if self.num_render_lines > (self.frame_height - gap) {
            // Render the last lines of the buffer
            self.render_index = self.num_render_lines - self.frame_height + gap;
        } else {
            // Can render the entire buffer anyway
            self.render_index = 0;
        }
    }

    pub fn move_down_lines(&mut self, num_lines: usize) {
        if let Some(res) = self.render_index.checked_add(num_lines) {
            if res >= self.num_render_lines {
                if self.num_render_lines > 0 {
                    self.render_index = self.num_render_lines - 1;
                } else {
                    self.render_index = 0;
                }
            } else {
                self.render_index = res;
            }
        }
    }

    pub fn move_up_lines(&mut self, num_lines: usize) {
        if let Some(res) = self.render_index.checked_sub(num_lines) {
            self.render_index = res;
        } else {
            self.render_index = 0;
        }
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> IntoIterator for &'a Renderer {
    type Item = usize;
    type IntoIter = LineIterator<'a>;

    fn into_iter(self) -> Self::IntoIter {
        LineIterator {
            renderer: self,
            offset: self.render_index,
        }
    }
}

pub struct LineIterator<'a> {
    renderer: &'a Renderer,
    offset: usize,
}

impl Iterator for LineIterator<'_> {
    type Item = usize;
    fn next(&mut self) -> Option<usize> {
        // See whether there are any indices at all
        self.renderer.render_indices.as_ref()?;

        let render_indices = self.renderer.render_indices.as_ref().unwrap();
        let render_index = match render_indices.get(self.offset) {
            Some(index) => {
                self.offset += 1;
                Some(*index)
            }
            None => None,
        };
        render_index
    }
}
