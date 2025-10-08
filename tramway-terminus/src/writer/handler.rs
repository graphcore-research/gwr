// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::writer::app::{App, AppState};

/// Handle user key events. Different paths will be taken depending on the
/// current state
pub fn handle_key_event(key_event: KeyEvent, app: &mut App) {
    match app.app_state() {
        AppState::HistorySelection => handle_key_event_history_selection(key_event, app),
        AppState::HistoryEditing => handle_key_event_history_edit(key_event, app),
        AppState::EditingSelectCommand => handle_key_event_editing(key_event, app),
        AppState::ShowHelp => {
            app.restore_app_state();
        }
        AppState::SaveFileName => handle_key_event_save_filename(key_event, app),
        AppState::SaveDescription => handle_key_event_save_description(key_event, app),
    }
}

fn handle_key_event_history_selection(key_event: KeyEvent, app: &mut App) {
    if key_event.modifiers == KeyModifiers::CONTROL {
        app.command_history().handle_key_event(key_event);
    } else {
        match key_event.code {
            KeyCode::Esc => {
                app.exit();
            }
            KeyCode::Enter => {
                app.set_app_state(AppState::HistoryEditing);
            }
            KeyCode::Tab | KeyCode::BackTab => {
                app.set_app_state(AppState::EditingSelectCommand);
            }
            KeyCode::Char('?') => {
                app.set_app_state(AppState::ShowHelp);
            }
            KeyCode::Char(' ') => {
                app.toggle_current_row_selected();
            }
            KeyCode::Char('w') => {
                app.set_app_state(AppState::SaveFileName);
            }
            _ => app.command_history().handle_key_event(key_event),
        }
    }
}

fn handle_key_event_history_edit(key_event: KeyEvent, app: &mut App) {
    if key_event.modifiers == KeyModifiers::CONTROL {
        app.get_current_history_string().handle_key_event(key_event);
    } else {
        match key_event.code {
            KeyCode::Esc | KeyCode::Enter => {
                app.set_app_state(AppState::HistorySelection);
            }
            KeyCode::Char('?') => {
                app.set_app_state(AppState::ShowHelp);
            }
            _ => app.get_current_history_string().handle_key_event(key_event),
        }
    }
}

fn handle_key_event_editing(key_event: KeyEvent, app: &mut App) {
    if key_event.modifiers == KeyModifiers::CONTROL {
        app.select_command().handle_key_event(key_event);
    } else {
        match key_event.code {
            KeyCode::Enter => {
                app.process_select_command();
                app.select_command().clear_value();
            }
            KeyCode::Esc => app.exit(),
            KeyCode::Char('?') => app.set_app_state(AppState::ShowHelp),
            KeyCode::Tab | KeyCode::BackTab => app.set_app_state(AppState::HistorySelection),
            _ => app.select_command().handle_key_event(key_event),
        }
    }
}

fn handle_key_event_save_filename(key_event: KeyEvent, app: &mut App) {
    if key_event.modifiers == KeyModifiers::CONTROL {
        app.recipe_filename().handle_key_event(key_event);
    } else {
        match key_event.code {
            KeyCode::Tab | KeyCode::BackTab => app.set_app_state(AppState::SaveDescription),
            KeyCode::Enter => {
                app.write_recipe();
                app.restore_app_state();
            }
            KeyCode::Esc => app.set_app_state(AppState::HistorySelection),
            _ => app.recipe_filename().handle_key_event(key_event),
        }
    }
}

fn handle_key_event_save_description(key_event: KeyEvent, app: &mut App) {
    if key_event.modifiers == KeyModifiers::CONTROL {
        app.recipe_description().handle_key_event(key_event);
    } else {
        match key_event.code {
            KeyCode::Tab | KeyCode::BackTab => app.set_app_state(AppState::SaveFileName),
            KeyCode::Enter => {
                app.write_recipe();
                app.restore_app_state();
            }
            KeyCode::Esc => app.set_app_state(AppState::HistorySelection),
            _ => app.recipe_description().handle_key_event(key_event),
        }
    }
}
