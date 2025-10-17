// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::cell::{RefCell, RefMut};
use std::fs;
use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use regex::Regex;

use crate::app_string::AppString;
use crate::recipe::Recipe;
use crate::runner::ui::{
    render_app_string, render_args, render_description, render_help_line, render_ingredients,
    render_messages_area, render_recipes,
};
use crate::vec_with_index::VecWithIndex;
use crate::{Draw, Logger};

/// Wrapper structure to carry extra information only useful within the
/// application
pub struct AppRecipe {
    name: String,
    path: PathBuf,
    recipe: Recipe,
}

pub struct AppLogger {
    /// Log messages
    pub messages: VecWithIndex<String>,
}

impl Default for AppLogger {
    fn default() -> Self {
        Self {
            messages: VecWithIndex::new(true),
        }
    }
}

impl Logger for RefMut<'_, AppLogger> {
    fn error(&mut self, message: &str) {
        self.messages.rows_mut().push(message.to_string());
    }

    fn info(&mut self, message: &str) {
        self.messages.rows_mut().push(message.to_string());
    }
}

impl AppRecipe {
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    #[must_use]
    pub fn recipe(&self) -> &Recipe {
        &self.recipe
    }
}

/// Derive the recipe name from the file
#[must_use]
fn name_from_path(file_path: &Path) -> String {
    match file_path.file_stem() {
        Some(name) => name.to_str().unwrap().to_string(),
        None => file_path.to_str().unwrap().to_string(),
    }
}

/// App holds the state of the application
pub struct App {
    /// Name of temporary file to be used
    tmp_root: PathBuf,

    /// Whether or not to keep the temporary files created
    keep_tmp: bool,

    /// Current value of the command box
    search_string: AppString,

    /// Current application state which controls behaviour
    app_state: AppState,

    /// Previous application state
    previous_app_state: AppState,

    /// List of recipes found
    recipes: Vec<AppRecipe>,

    /// List of argument values
    arg_values: VecWithIndex<AppString>,

    /// Whether or not the application is still running or should quit
    running: bool,

    /// The index of the current row in the recipe
    matching_recipe_index: VecWithIndex<usize>,

    /// The rows of the ingredients box
    ingredient_rows: VecWithIndex<String>,

    /// Handler for log messages. Needs to be in RefCell in order to be used by
    /// recipe.execute().
    logger: RefCell<AppLogger>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum AppState {
    EditingSearchString,
    RecipeSelection,
    ArgSelection,
    EditingArg,
    IngedientSelection,
    ShowHelp,
    ViewMessages,
}

#[must_use]
fn build_recipe_list(recipes_folder: &str, logger: &mut impl Logger) -> Vec<AppRecipe> {
    let mut lines = Vec::new();
    let paths = match fs::read_dir(recipes_folder) {
        Ok(paths) => paths,
        Err(e) => {
            logger.error(&format!("Failed to read folder '{recipes_folder}'\n{e}"));
            return lines;
        }
    };

    for path in paths {
        let path = path.unwrap();
        let recipe = match Recipe::new_from_file(&path.path()) {
            Ok(recipe) => recipe,
            Err(e) => {
                logger.error(format!("{e}").as_str());
                continue;
            }
        };
        let app_recipe = AppRecipe {
            name: name_from_path(&path.path()),
            path: path.path(),
            recipe,
        };
        lines.push(app_recipe);
    }
    lines
}

impl App {
    #[must_use]
    pub fn new(recipe_folder: &str, tmp_root: &Path, keep_tmp: bool) -> Self {
        let mut app = Self {
            tmp_root: PathBuf::from(tmp_root),
            keep_tmp,
            search_string: AppString::new(""),
            app_state: AppState::RecipeSelection,
            previous_app_state: AppState::EditingSearchString,
            recipes: Vec::new(),
            arg_values: VecWithIndex::default(),
            running: true,
            matching_recipe_index: VecWithIndex::default(),
            ingredient_rows: VecWithIndex::default(),
            logger: RefCell::new(AppLogger::default()),
        };
        {
            let mut logger = app.logger.borrow_mut();
            app.recipes = build_recipe_list(recipe_folder, &mut logger);
        }
        app.process_search();
        app
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn exit(&mut self) {
        self.running = false;
    }

    #[must_use]
    pub fn app_state(&self) -> AppState {
        self.app_state
    }

    pub fn set_app_state(&mut self, app_state: AppState) {
        self.previous_app_state = self.app_state;
        self.app_state = app_state;
    }

    pub fn restore_app_state(&mut self) {
        self.app_state = self.previous_app_state;
    }

    pub fn run_current_recipe(&mut self) {
        let recipe_index = self.recipes_row_index().index();
        let recipe = &mut self.recipes[recipe_index].recipe;
        let mut logger = self.logger.borrow_mut();
        recipe.execute(self.tmp_root.as_path(), self.keep_tmp, &mut logger);
    }

    pub fn logger(&mut self) -> RefMut<'_, AppLogger> {
        self.logger.borrow_mut()
    }

    #[must_use]
    pub fn search_string(&mut self) -> &mut AppString {
        &mut self.search_string
    }

    #[must_use]
    pub fn get_current_arg_string(&mut self) -> &mut AppString {
        self.arg_values.selected_mut()
    }

    /// Update the argument in the recipe from the ArgString that has been
    /// edited
    pub fn update_arg_after_edit(&mut self) {
        let recipe_index = self.recipes_row_index().index();

        let value = self.arg_values.selected().value();
        let arg_index = self.arg_values.index();
        self.recipes[recipe_index]
            .recipe
            .set_argument_value(arg_index, value);
    }

    #[must_use]
    pub fn recipes_row_index(&mut self) -> &mut VecWithIndex<usize> {
        &mut self.matching_recipe_index
    }

    #[must_use]
    pub fn arg_values(&mut self) -> &mut VecWithIndex<AppString> {
        &mut self.arg_values
    }

    #[must_use]
    pub fn ingredient_rows(&mut self) -> &mut VecWithIndex<String> {
        &mut self.ingredient_rows
    }

    /// Update the list of matching recipes given the new search string
    pub fn process_search(&mut self) {
        let search_string = self.search_string.value();
        let search_re = match Regex::new(search_string) {
            Ok(re) => re,
            Err(e) => {
                self.logger.borrow_mut().error(&format!(
                    "Failed to compile search regex from '{search_string}'\n{e}"
                ));
                return;
            }
        };

        self.matching_recipe_index.rows_mut().clear();
        let mut recipes_row_index = None;
        for (i, app_recipe) in self.recipes.iter().enumerate() {
            if app_recipe.recipe.matches(&search_re) {
                self.matching_recipe_index.rows_mut().push(i);

                // Find the highest matching index close to the current row index
                if recipes_row_index.is_none() || i <= self.matching_recipe_index.index() {
                    recipes_row_index = Some(i);
                }
            }
        }

        // Update
        if let Some(index) = recipes_row_index {
            self.matching_recipe_index.set_index(index);
        }

        self.update_args_and_ingredients();
    }

    pub fn update_args_and_ingredients(&mut self) {
        // Always clear the existing arg/ingredients rows
        self.ingredient_rows.clear();
        self.arg_values.clear();

        if self.matching_recipe_index.rows().is_empty() {
            // Nothing more to do if there are no matching recipes
            return;
        }

        let matching_row_index = self.matching_recipe_index.index();
        let selected_recipe_index = self.matching_recipe_index.rows()[matching_row_index];

        let selected_recipe = match self.recipes.get(selected_recipe_index) {
            Some(recipe) => recipe,
            None => return,
        };

        for arg in selected_recipe.recipe().arguments() {
            match arg.value() {
                Some(value) => self.arg_values.rows_mut().push(AppString::new(value)),
                None => self.arg_values.rows_mut().push(AppString::new("")),
            }
        }

        for ingredient in selected_recipe.recipe().ingredients() {
            self.ingredient_rows
                .rows_mut()
                .push(ingredient.command().to_string());
        }
    }
}

impl Draw for App {
    fn draw(&self, frame: &mut Frame) {
        if self.app_state == AppState::ShowHelp {
            crate::runner::ui::render_help(frame, frame.area());
            return;
        }

        let vertical = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Min(1),
            Constraint::Length(5),
        ]);
        let [
            help_area,
            search_string_area,
            recipes_area,
            description_area,
            args_area,
            ingredients_area,
            messages_area,
        ] = vertical.areas(frame.area());

        render_help_line(frame, help_area, &self.app_state);
        render_app_string(
            frame,
            search_string_area,
            "Selection Command",
            &self.search_string,
            self.app_state == AppState::EditingSearchString,
        );
        render_recipes(
            frame,
            recipes_area,
            &self.recipes,
            &self.matching_recipe_index,
            &self.app_state,
        );

        let selected_recipe = if self.matching_recipe_index.rows().is_empty() {
            None
        } else {
            let current_index =
                self.matching_recipe_index.rows()[self.matching_recipe_index.index()];
            Some(self.recipes[current_index].recipe())
        };
        render_description(
            frame,
            description_area,
            selected_recipe,
            self.arg_values.index(),
            self.ingredient_rows.index(),
            &self.app_state,
        );
        render_args(
            frame,
            args_area,
            selected_recipe,
            &self.arg_values,
            &self.app_state,
        );
        render_ingredients(
            frame,
            ingredients_area,
            &self.ingredient_rows,
            &self.app_state,
        );
        let logger = self.logger.borrow_mut();
        render_messages_area(frame, messages_area, &logger.messages, &self.app_state);
    }
}
