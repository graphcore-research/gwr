// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use regex::Regex;

use crate::app::{CHUNK_SIZE, EventLine, INITIAL_SIZE};
use crate::renderer::Renderer;

pub struct Filter {
    id_re: Regex,

    pub filter: String,
    pub search: String,
    pub search_cursor_pos: usize,
    pub use_regex: bool,

    notify_filter: Sender<()>,

    id_to_name: Option<HashMap<u64, String>>,
    id_to_name_updates: Vec<HashMap<u64, String>>,
}

struct SearchState {
    use_regex: bool,
    search_re: Option<Regex>,
    search: String,
    id_to_name: HashMap<u64, String>,
    filter_id: Option<u64>,
}

impl SearchState {
    fn text_matches(&self, text: &str) -> bool {
        if self.search.is_empty() {
            return self.filter_id.is_none();
        }

        if self.use_regex && self.search_re.is_some() {
            self.search_re.as_ref().unwrap().is_match(text)
        } else {
            text.contains(self.search.as_str())
        }
    }

    fn id_matches(&self, id: &u64) -> bool {
        if let Some(filter_id) = &self.filter_id {
            return filter_id == id;
        }

        if let Some(name) = self.id_to_name.get(id) {
            self.text_matches(name)
        } else {
            false
        }
    }

    pub fn search_matches(&self, line: &EventLine) -> bool {
        match line {
            EventLine::Create { id, time: _ } => self.id_matches(id),
            EventLine::Connect {
                from_id,
                to_id,
                time: _,
            } => self.id_matches(from_id) || self.id_matches(to_id),
            EventLine::Enter {
                id,
                entered,
                fullness: _,
                time: _,
            } => self.id_matches(id) || self.id_matches(entered),
            EventLine::Exit {
                id,
                exited,
                fullness: _,
                time: _,
            } => self.id_matches(id) || self.id_matches(exited),
            EventLine::Log {
                level: _,
                id,
                msg,
                time: _,
            } => self.id_matches(id) || self.text_matches(msg),
        }
    }
}

impl Filter {
    pub fn new(notify_filter: Sender<()>) -> Self {
        Self {
            id_re: Regex::new(r"id=(?<id>\d+)").unwrap(),
            notify_filter,

            id_to_name: Some(HashMap::with_capacity(INITIAL_SIZE)),
            id_to_name_updates: Vec::new(),

            filter: String::new(),
            search: String::new(),
            search_cursor_pos: 0,
            use_regex: true,
        }
    }

    /// Add id_to_name updates.
    ///
    /// They can either be applied right now, or stored to be applied when the
    /// HashMap is restored. It is taken out at times for the filter thread to
    /// use. When it is restored the updates will be applied then.
    pub fn extend_id_to_name(&mut self, update: HashMap<u64, String>) {
        if let Some(id_to_name) = &mut self.id_to_name {
            id_to_name.extend(update);
            self.notify_filter.send(()).unwrap();
        } else {
            self.id_to_name_updates.push(update);
        }
    }

    pub fn push_search_char(&mut self, c: char) {
        self.search.insert(self.search_cursor_pos, c);
        self.search_cursor_pos += 1;
        self.notify_filter.send(()).unwrap();
    }

    pub fn del_search_char(&mut self) {
        if self.search_cursor_pos < self.search.len() {
            self.search.remove(self.search_cursor_pos);
            self.notify_filter.send(()).unwrap();
        }
    }

    pub fn backspace_search_char(&mut self) {
        if self.search_cursor_pos > 0 {
            if self.search_cursor_pos >= self.search.len() {
                self.search.pop();
                self.search_cursor_pos -= 1;
            } else {
                self.search.remove(self.search_cursor_pos - 1);
                self.search_cursor_pos -= 1;
            }
            self.notify_filter.send(()).unwrap();
        }
    }

    pub fn set(&mut self, new_filter_string: &str) {
        self.search = self.search[self.search_cursor_pos..].to_owned();
        self.search_cursor_pos = 0;
        self.search = new_filter_string.to_owned();
        self.notify_filter.send(()).unwrap();
    }

    pub fn clear_to_start(&mut self) {
        self.search = self.search[self.search_cursor_pos..].to_owned();
        self.search_cursor_pos = 0;
        self.notify_filter.send(()).unwrap();
    }

    pub fn clear_search(&mut self) {
        self.search.clear();
        self.search_cursor_pos = 0;
        self.notify_filter.send(()).unwrap();
    }

    pub fn move_search_cursor_left(&mut self) {
        if self.search_cursor_pos > 0 {
            self.search_cursor_pos -= 1;
        }
    }

    pub fn move_search_cursor_right(&mut self) {
        if self.search_cursor_pos < self.search.len() {
            self.search_cursor_pos += 1;
        }
    }

    pub fn move_search_cursor_start(&mut self) {
        self.search_cursor_pos = 0;
    }

    pub fn move_search_cursor_end(&mut self) {
        self.search_cursor_pos = self.search.len();
    }

    pub fn toggle_regex(&mut self) {
        self.use_regex = !self.use_regex;
        self.notify_filter.send(()).unwrap();
    }

    pub fn regex_enabled(&self) -> bool {
        self.use_regex
    }

    fn start_search(&mut self) -> SearchState {
        let mut filter_id = None;
        let mut search = self.search.to_owned();
        if let Some(e) = self.id_re.captures(&self.search) {
            let id_str = e.name("id").unwrap().as_str();
            if let Ok(id) = id_str.parse() {
                filter_id = Some(id);

                let to_remove = format!("id={id_str}");
                search = search.replace(to_remove.as_str(), "");
            }
        }
        search = search.trim().to_owned();

        let mut search_re = None;
        if self.use_regex
            && let Ok(re) = Regex::new(search.as_str())
        {
            search_re = Some(re);
        }

        SearchState {
            use_regex: self.use_regex,
            filter_id,
            search_re,
            search,
            id_to_name: self.id_to_name.take().unwrap(),
        }
    }

    fn search_done(&mut self, mut id_to_name: HashMap<u64, String>) {
        for update in self.id_to_name_updates.drain(..) {
            id_to_name.extend(update);
        }
        self.id_to_name = Some(id_to_name);
    }

    /// Returns whether the user has specified a ID
    pub fn id_defined(&self) -> bool {
        self.id_re.captures(&self.search).is_some()
    }
}

/// Run a background thread that keeps the render lines updated.
///
/// This thread is notified whenever there is a change of the search string
/// or the lines to be filtered.
pub fn start_background_filter(
    receiver: Receiver<()>,
    renderer: Arc<Mutex<Renderer>>,
    filter: Arc<Mutex<Filter>>,
) {
    thread::spawn(move || {
        loop {
            if receiver.recv().is_err() {
                return;
            }

            // Drain any notifications that have built up
            while receiver.try_recv().is_ok() {}

            // Get the current filter state for the duration of the filtering process.
            let search_state = filter.lock().unwrap().start_search();

            let mut matching_indices = Vec::with_capacity(INITIAL_SIZE);
            let mut chunk_index = 0;
            loop {
                let chunk_offset = chunk_index * CHUNK_SIZE;

                // Take a chunk of lines out of the renderer to be filtered.
                let chunk = renderer.lock().unwrap().take_chunk(chunk_index);
                if chunk.is_none() {
                    break;
                }

                let block_ref = chunk.as_ref().unwrap();

                for (index, line) in block_ref.iter().enumerate() {
                    if search_state.search_matches(line) {
                        matching_indices.push(index + chunk_offset);
                    }
                }

                // Restore the chunk of lines
                renderer.lock().unwrap().restore_chunk(chunk_index, chunk);
                chunk_index += 1;

                // Somthing has notified us, so break out and start again.
                if receiver.try_recv().is_ok() {
                    break;
                }
            }
            renderer
                .lock()
                .unwrap()
                .set_render_indices(matching_indices);

            filter.lock().unwrap().search_done(search_state.id_to_name);
        }
    });
}
