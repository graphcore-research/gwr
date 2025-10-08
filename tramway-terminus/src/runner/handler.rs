// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::runner::app::{App, AppState};

/// Handle user key events. Different paths will be taken depending on the
/// current state
pub fn handle_key_event(key_event: KeyEvent, app: &mut App) {
    match app.app_state() {
        AppState::RecipeSelection => handle_key_event_recipe_selection(key_event, app),
        AppState::ArgSelection => handle_key_event_arg_selection(key_event, app),
        AppState::IngedientSelection => handle_key_event_ingredient_selection(key_event, app),
        AppState::EditingSearchString => handle_key_event_edit_search(key_event, app),
        AppState::EditingArg => handle_key_event_edit_arg(key_event, app),
        AppState::ShowHelp => app.restore_app_state(),
        AppState::ViewMessages => handle_key_event_view_messages(key_event, app),
    }
}

fn handle_key_event_recipe_selection(key_event: KeyEvent, app: &mut App) {
    if !handle_key_event_row_selection(
        key_event,
        app,
        AppState::EditingSearchString,
        AppState::ArgSelection,
    ) {
        if key_event.code == KeyCode::Enter {
            app.run_current_recipe();
        } else {
            app.recipes_row_index().handle_key_event(key_event);
            app.update_args_and_ingredients();
        }
    }
}

fn handle_key_event_arg_selection(key_event: KeyEvent, app: &mut App) {
    if !handle_key_event_row_selection(
        key_event,
        app,
        AppState::RecipeSelection,
        AppState::IngedientSelection,
    ) {
        if key_event.code == KeyCode::Enter {
            app.set_app_state(AppState::EditingArg);
        } else {
            app.arg_values().handle_key_event(key_event);
        }
    }
}

fn handle_key_event_ingredient_selection(key_event: KeyEvent, app: &mut App) {
    if !handle_key_event_row_selection(
        key_event,
        app,
        AppState::ArgSelection,
        AppState::ViewMessages,
    ) {
        app.ingredient_rows().handle_key_event(key_event);
    }
}

fn handle_key_event_row_selection(
    key_event: KeyEvent,
    app: &mut App,
    prev: AppState,
    next: AppState,
) -> bool {
    match key_event.code {
        KeyCode::Esc => app.exit(),
        KeyCode::BackTab => app.set_app_state(prev),
        KeyCode::Tab => app.set_app_state(next),
        KeyCode::Char('?') => app.set_app_state(AppState::ShowHelp),
        _ => return false,
    }
    true
}

fn handle_key_event_edit_search(key_event: KeyEvent, app: &mut App) {
    if key_event.modifiers == KeyModifiers::CONTROL {
        app.search_string().handle_key_event(key_event);
        app.process_search();
    } else {
        match key_event.code {
            KeyCode::Esc => app.exit(),
            KeyCode::Char('?') => app.set_app_state(AppState::ShowHelp),
            KeyCode::Tab => app.set_app_state(AppState::RecipeSelection),
            _ => {
                app.search_string().handle_key_event(key_event);
                app.process_search();
            }
        }
    }
}

fn handle_key_event_edit_arg(key_event: KeyEvent, app: &mut App) {
    if key_event.modifiers == KeyModifiers::CONTROL {
        app.get_current_arg_string().handle_key_event(key_event);
        app.update_arg_after_edit();
    } else {
        match key_event.code {
            KeyCode::Esc | KeyCode::Enter => {
                app.set_app_state(AppState::ArgSelection);
            }
            KeyCode::Char('?') => {
                app.set_app_state(AppState::ShowHelp);
            }
            _ => {
                app.get_current_arg_string().handle_key_event(key_event);
                app.update_arg_after_edit();
            }
        }
    }
}

fn handle_key_event_view_messages(key_event: KeyEvent, app: &mut App) {
    match key_event.code {
        KeyCode::Esc => app.exit(),
        KeyCode::BackTab => app.set_app_state(AppState::IngedientSelection),
        KeyCode::Tab => app.set_app_state(AppState::EditingSearchString),
        KeyCode::Char('?') => app.set_app_state(AppState::ShowHelp),
        _ => app.logger().messages.handle_key_event(key_event),
    }
}
