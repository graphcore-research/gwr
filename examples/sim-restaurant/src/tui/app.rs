// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use crossterm::event::{KeyCode, KeyEvent};

use crate::recording::{PlotStat, RecordedSimulation};

const PLAYBACK_STEPS: [usize; 6] = [1, 2, 4, 8, 16, 32];
const WINDOW_SIZES: [u64; 7] = [30, 60, 120, 300, 600, 1800, 3600];

#[derive(Clone, Copy, Debug)]
pub struct PlotConfig {
    pub stat: PlotStat,
    pub windowed: bool,
}

#[derive(Debug)]
pub struct App {
    pub recording: RecordedSimulation,
    pub frame_index: usize,
    pub playing: bool,
    pub selected_plot: usize,
    pub plots: [PlotConfig; 4],
    pub speed_index: usize,
    pub window_size_index: usize,
}

impl App {
    #[must_use]
    pub fn new(recording: RecordedSimulation) -> Self {
        Self {
            recording,
            frame_index: 0,
            playing: true,
            selected_plot: 0,
            plots: [
                PlotConfig {
                    stat: PlotStat::TillQueueLen,
                    windowed: false,
                },
                PlotConfig {
                    stat: PlotStat::ActiveTillWorkers,
                    windowed: false,
                },
                PlotConfig {
                    stat: PlotStat::KitchenQueueLen,
                    windowed: false,
                },
                PlotConfig {
                    stat: PlotStat::ActiveKitchenWorkers,
                    windowed: false,
                },
            ],
            speed_index: 2,
            window_size_index: 3,
        }
    }

    #[must_use]
    pub fn current_tick(&self) -> u64 {
        self.recording
            .timeline
            .get(self.frame_index)
            .map_or(0, |point| point.tick)
    }

    #[must_use]
    pub fn current_snapshot(&self) -> &crate::recording::SimulationSnapshot {
        &self.recording.timeline[self.frame_index].snapshot
    }

    #[must_use]
    pub fn speed_label(&self) -> String {
        format!("{}x", PLAYBACK_STEPS[self.speed_index])
    }

    #[must_use]
    pub fn window_size_ticks(&self) -> u64 {
        WINDOW_SIZES[self.window_size_index]
    }

    pub fn advance(&mut self) {
        if self.frame_index + 1 >= self.recording.timeline.len() {
            self.playing = false;
            return;
        }

        let step = PLAYBACK_STEPS[self.speed_index];
        self.frame_index = (self.frame_index + step).min(self.recording.timeline.len() - 1);
    }

    pub fn rewind(&mut self) {
        let step = PLAYBACK_STEPS[self.speed_index];
        self.frame_index = self.frame_index.saturating_sub(step);
    }

    #[must_use]
    pub fn recent_events(&self, count: usize) -> Vec<&crate::recording::TimelineEvent> {
        let current_tick = self.current_tick();
        self.recording
            .events
            .iter()
            .filter(|event| event.tick <= current_tick)
            .rev()
            .take(count)
            .collect()
    }

    pub fn handle_tick(&mut self) {
        if self.playing {
            self.advance();
        }
    }

    #[must_use]
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char(' ') => {
                self.playing = !self.playing;
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.playing = false;
                self.rewind();
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.playing = false;
                self.advance();
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.playing = false;
                self.frame_index = 0;
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.playing = false;
                self.frame_index = self.recording.timeline.len().saturating_sub(1);
            }
            KeyCode::Char('[') | KeyCode::Char('-') => {
                self.speed_index = self.speed_index.saturating_sub(1);
            }
            KeyCode::Char(']') | KeyCode::Char('+') => {
                self.speed_index = (self.speed_index + 1).min(PLAYBACK_STEPS.len() - 1);
            }
            KeyCode::Char('{') => {
                self.window_size_index = self.window_size_index.saturating_sub(1);
            }
            KeyCode::Char('}') => {
                self.window_size_index = (self.window_size_index + 1).min(WINDOW_SIZES.len() - 1);
            }
            KeyCode::Char('1') => self.selected_plot = 0,
            KeyCode::Char('2') => self.selected_plot = 1,
            KeyCode::Char('3') => self.selected_plot = 2,
            KeyCode::Char('4') => self.selected_plot = 3,
            KeyCode::Char('w') => {
                let plot = &mut self.plots[self.selected_plot];
                plot.windowed = !plot.windowed;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.plots[self.selected_plot].stat =
                    self.plots[self.selected_plot].stat.previous();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.plots[self.selected_plot].stat = self.plots[self.selected_plot].stat.next();
            }
            _ => {}
        }
        false
    }
}
