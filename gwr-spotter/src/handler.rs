// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppResult, InputState};

pub const TOGGLE_RE: KeyCode = KeyCode::F(1);

/// Handles the key events and updates the state of [`App`].
pub fn handle_key_events(key_event: KeyEvent, app: &mut App) -> AppResult<()> {
    match app.state() {
        InputState::Default => handle_key_events_default(key_event, app),
        InputState::Goto => handle_key_events_goto(key_event, app),
        InputState::Help => handle_key_events_help(key_event, app),
        InputState::Numbers => handle_key_events_numbers(key_event, app),
        InputState::Search => handle_key_events_search(key_event, app),
    }
}

pub fn handle_key_events_default(key_event: KeyEvent, app: &mut App) -> AppResult<()> {
    if key_event.modifiers == KeyModifiers::CONTROL {
        match key_event.code {
            // Exit application on `Ctrl-C`
            KeyCode::Char('c') | KeyCode::Char('C') => {
                app.quit();
            }
            KeyCode::Char('f') => {
                app.move_down_block();
            }
            KeyCode::Char('u') => {
                app.move_up_block();
            }

            _ => {}
        }
    } else {
        match key_event.code {
            // Exit application on `q`
            KeyCode::Char('q') => {
                app.quit();
            }

            // Move around the log
            KeyCode::Down | KeyCode::Char('j') => {
                app.move_down_lines(1);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.move_up_lines(1);
            }

            KeyCode::PageDown => {
                app.move_down_block();
            }
            KeyCode::PageUp => {
                app.move_up_block();
            }

            // Configure rendering
            KeyCode::Char('f') => {
                app.toggle_plot_fullness();
            }
            KeyCode::Char('n') => {
                app.toggle_print_names();
            }
            KeyCode::Char('p') => {
                app.toggle_print_packets();
            }
            KeyCode::Char('t') => {
                app.toggle_print_times();
            }

            KeyCode::Char('g') => {
                app.set_state(InputState::Goto);
            }

            KeyCode::Char('G') => {
                app.move_bottom();
            }

            KeyCode::Char('/') => {
                app.set_state(InputState::Search);
            }

            TOGGLE_RE => {
                app.filter.lock().unwrap().toggle_regex();
            }

            KeyCode::Char('?') => {
                app.set_state(InputState::Help);
            }

            KeyCode::Char(c) => {
                if c.is_ascii_digit() {
                    app.push_number_char(c);
                    app.set_state(InputState::Numbers);
                }
            }

            _ => {}
        }
    }
    Ok(())
}

pub fn handle_key_events_goto(key_event: KeyEvent, app: &mut App) -> AppResult<()> {
    match key_event.code {
        KeyCode::Char('g') => {
            app.move_top();
        }

        _ => {
            app.set_state(InputState::Default);
            return handle_key_events_default(key_event, app);
        }
    }

    app.set_state(InputState::Default);
    Ok(())
}

pub fn handle_key_events_help(_key_event: KeyEvent, app: &mut App) -> AppResult<()> {
    app.set_state(InputState::Default);
    Ok(())
}

pub fn handle_key_events_search(key_event: KeyEvent, app: &mut App) -> AppResult<()> {
    if key_event.modifiers == KeyModifiers::CONTROL {
        match key_event.code {
            KeyCode::Char('u') => {
                app.filter.lock().unwrap().clear_to_start();
            }
            KeyCode::Char('a') => {
                app.filter.lock().unwrap().move_search_cursor_start();
            }
            KeyCode::Char('e') => {
                app.filter.lock().unwrap().move_search_cursor_end();
            }

            _ => {
                return handle_key_events_default(key_event, app);
            }
        }
    } else {
        match key_event.code {
            KeyCode::Char(c) => {
                app.filter.lock().unwrap().push_search_char(c);
            }
            KeyCode::Backspace => {
                app.filter.lock().unwrap().backspace_search_char();
            }
            KeyCode::Delete => {
                app.filter.lock().unwrap().del_search_char();
            }
            KeyCode::Enter => {
                app.set_state(InputState::Default);
            }
            KeyCode::Esc => {
                app.filter.lock().unwrap().clear_search();
                app.set_state(InputState::Default);
            }

            KeyCode::Left => {
                app.filter.lock().unwrap().move_search_cursor_left();
            }
            KeyCode::Right => {
                app.filter.lock().unwrap().move_search_cursor_right();
            }

            _ => {
                return handle_key_events_default(key_event, app);
            }
        }
    }

    Ok(())
}

pub fn handle_key_events_numbers(key_event: KeyEvent, app: &mut App) -> AppResult<()> {
    match key_event.code {
        KeyCode::Char('g') | KeyCode::Char('G') => {
            app.move_to_number();
            app.set_state(InputState::Default);
        }
        KeyCode::Char('%') => {
            app.move_to_percent();
            app.set_state(InputState::Default);
        }
        KeyCode::Char(c) => {
            if c.is_ascii_digit() {
                app.push_number_char(c);
            }
        }
        KeyCode::Enter => {
            app.move_down_n();
            app.set_state(InputState::Default);
        }
        KeyCode::Esc => {
            app.clear_numbers();
            app.set_state(InputState::Default);
        }

        _ => {
            return handle_key_events_default(key_event, app);
        }
    }
    Ok(())
}
