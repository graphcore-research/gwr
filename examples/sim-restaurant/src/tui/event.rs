// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event as CrosstermEvent, KeyEvent};

pub type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Tick,
    Key(KeyEvent),
    Resize(u16, u16),
}

#[derive(Debug)]
pub struct EventHandler {
    receiver: mpsc::Receiver<Event>,
    _handler: thread::JoinHandle<()>,
}

impl EventHandler {
    #[must_use]
    pub fn new(tick_rate_ms: u64) -> Self {
        let tick_rate = Duration::from_millis(tick_rate_ms);
        let (sender, receiver) = mpsc::channel();
        let handler = thread::spawn(move || {
            let mut last_tick = Instant::now();
            loop {
                let timeout = tick_rate
                    .checked_sub(last_tick.elapsed())
                    .unwrap_or(tick_rate);

                if event::poll(timeout).expect("no events available") {
                    match event::read().expect("unable to read event") {
                        CrosstermEvent::Key(key) => sender.send(Event::Key(key)),
                        CrosstermEvent::Resize(width, height) => {
                            sender.send(Event::Resize(width, height))
                        }
                        _ => continue,
                    }
                    .expect("failed to send terminal event");
                }

                if last_tick.elapsed() >= tick_rate {
                    sender.send(Event::Tick).expect("failed to send tick event");
                    last_tick = Instant::now();
                }
            }
        });

        Self {
            receiver,
            _handler: handler,
        }
    }

    pub fn next(&self) -> AppResult<Event> {
        Ok(self.receiver.recv()?)
    }
}
