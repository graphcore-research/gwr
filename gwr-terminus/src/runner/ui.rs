// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use ratatui::Frame;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, Paragraph};

use crate::app_string::AppString;
use crate::block_style;
use crate::recipe::Recipe;
use crate::runner::app::{AppRecipe, AppState};
use crate::vec_with_index::VecWithIndex;

struct HelpRender<'a> {
    style_header: Style,
    style_command: Style,
    style_text: Style,
    indent: &'static str,
    pub lines: Vec<Line<'a>>,
}

impl<'a> HelpRender<'a> {
    fn new() -> Self {
        Self {
            style_header: Style::default().add_modifier(Modifier::BOLD),
            style_command: Style::default().bg(Color::Blue).fg(Color::White),
            style_text: Style::default(),
            indent: "  ",
            lines: Vec::new(),
        }
    }

    fn add_header(&mut self, header: &'a str, extra: Vec<&'a str>) {
        self.add_blank_line();
        self.lines
            .push(Line::from(vec![Span::styled(header, self.style_header)]));
        if !extra.is_empty() {
            self.add_blank_line();
            for line in extra {
                self.lines.push(Line::from(vec![
                    Span::from(self.indent),
                    Span::from(self.indent),
                    Span::styled(line, self.style_text),
                ]));
            }
        }
        self.add_blank_line();
    }

    fn add_command_help_line(&mut self, command: &'a str, help: &'a str) {
        self.lines.push(Line::from(vec![
            Span::from(self.indent),
            Span::styled(command, self.style_command),
            Span::styled(format!(": {help}"), self.style_text),
        ]));
    }

    fn add_blank_line(&mut self) {
        self.lines.push(Line::from(vec![Span::from("")]));
    }
}

pub fn render_help(frame: &mut Frame, area: Rect) {
    let mut renderer = HelpRender::new();

    renderer.add_header(
        "Overview:",
        vec![
            "This is a utility for finding and running recipes.",
            "You are presented with the list of recipes that match the search string. ",
            "As you edit the search string the list of recipes is filtered by those that ",
            "have a match in their description or commands.",
        ],
    );

    renderer.add_header(
        "Edit search:",
        vec!["The tool starts in the search string editor."],
    );
    renderer.add_command_help_line("<Tab>", "move to selection window");
    renderer.add_command_help_line("<Esc>", "exit application");
    renderer.add_command_help_line("left-arrow", "move left in command text");
    renderer.add_command_help_line("right-arrow", "move right in command text");
    renderer.add_command_help_line("ctrl+a", "move to start of command text");
    renderer.add_command_help_line("ctrl+e", "move to end of command text");
    renderer.add_command_help_line("ctrl+u", "clear the command text");

    renderer.add_header(
        "Recipe selection:",
        vec!["Allows you to choose a recipe to run."],
    );
    renderer.add_command_help_line("<Tab>", "move to search string editor");
    renderer.add_command_help_line("<Esc>", "exit application");
    renderer.add_command_help_line("<Up>,k", "move up in the history");
    renderer.add_command_help_line("<Down>,j", "move down in the history");
    renderer.add_command_help_line("<Enter>", "run the recipe");
    renderer.add_command_help_line("<PgUp>/<PgDn>", "move up/down a block of recipes");
    renderer.add_command_help_line("ctrl+u/ctrl+f", "same as <PgUp>/<PgDn>");

    frame.render_widget(
        Paragraph::new(renderer.lines)
            .block(
                Block::default()
                    .title("Help")
                    .title_alignment(Alignment::Left)
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .style(Style::default().fg(Color::Cyan).bg(Color::Black))
            .alignment(Alignment::Left),
        area,
    );
}

pub fn render_help_line(frame: &mut Frame, help_area: Rect, app_state: &AppState) {
    let prefix = match app_state {
        AppState::EditingSearchString => "Editing Search Command: ".bold(),
        AppState::RecipeSelection => "Select Recipe: ".bold(),
        AppState::ArgSelection => "Select Argument: ".bold(),
        AppState::EditingArg => "Edit Argument: ".bold(),
        AppState::IngedientSelection => "Select Ingredient: ".bold(),
        AppState::ShowHelp => "Help: ".bold(),
        AppState::ViewMessages => "Messages: ".bold(),
    };
    let mut msg = vec![
        prefix,
        "Press ".into(),
        "Tab".bold(),
        " to change windows, ".into(),
        "?".bold(),
        " for help, ".into(),
        "Esc".bold(),
        " to exit.".into(),
    ];
    let suffix = match app_state {
        AppState::RecipeSelection => {
            vec![" Enter".bold(), " to run the selected recipe.".into()]
        }
        _ => vec![],
    };
    msg.extend_from_slice(&suffix);
    let text = Text::from(Line::from(msg));
    let help_message = Paragraph::new(text);
    frame.render_widget(help_message, help_area);
}

pub fn render_app_string(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    app_string: &AppString,
    editing: bool,
) {
    let input = Paragraph::new(app_string.value())
        .style(block_style(editing))
        .block(Block::bordered().title(title));
    frame.render_widget(input, area);

    if editing {
        // Show the cursor
        frame.set_cursor_position(Position::new(
            area.x + app_string.character_index() as u16 + 1,
            area.y + 1,
        ));
    }
}

pub fn render_recipes(
    frame: &mut Frame,
    recipes_area: Rect,
    recipes: &[AppRecipe],
    matching_recipe_indices: &VecWithIndex<usize>,
    app_state: &AppState,
) {
    let middle = recipes_area.height / 2;
    let current_index = matching_recipe_indices.index();
    let start_offset = current_index.saturating_sub(middle as usize);

    let recipe_lines: Vec<ListItem> = matching_recipe_indices
        .rows()
        .iter()
        .skip(start_offset)
        .take(recipes_area.height as usize)
        .map(|index| {
            let recipe = &recipes[*index];

            let mut style = Style::default();
            if *index == current_index {
                style = style.bold();
            }

            let content = Line::from(Span::raw(recipe.name().to_string())).style(style);
            ListItem::new(content)
        })
        .collect();

    let recipes = List::new(recipe_lines)
        .block(Block::bordered().title("Recipes"))
        .style(block_style(app_state == &AppState::RecipeSelection));
    frame.render_widget(recipes, recipes_area);
}

pub fn render_description(
    frame: &mut Frame,
    description_area: Rect,
    selected_recipe: Option<&Recipe>,
    args_index: usize,
    ingredient_index: usize,
    app_state: &AppState,
) {
    if let Some(selected_recipe) = selected_recipe {
        let line = match app_state {
            &AppState::RecipeSelection
            | &AppState::EditingSearchString
            | &AppState::ShowHelp
            | &AppState::ViewMessages => Line::from(selected_recipe.description()),
            &AppState::ArgSelection | &AppState::EditingArg => {
                match selected_recipe.arguments().get(args_index) {
                    Some(arg) => Line::from(arg.comment()),
                    None => Line::from(""),
                }
            }
            &AppState::IngedientSelection => {
                match selected_recipe.ingredients().get(ingredient_index) {
                    Some(ingredient) => Line::from(ingredient.comment()),
                    None => Line::from(""),
                }
            }
        };
        let text = Text::from(line);
        let description = Paragraph::new(text);
        frame.render_widget(description, description_area);
    }
}

pub fn render_args(
    frame: &mut Frame,
    args_area: Rect,
    selected_recipe: Option<&Recipe>,
    arg_values: &VecWithIndex<AppString>,
    app_state: &AppState,
) {
    let middle = args_area.height / 2;
    let current_index = arg_values.index();
    let start_offset = current_index.saturating_sub(middle as usize);

    let arg_lines: Vec<ListItem> = match selected_recipe {
        Some(selected_recipe) => arg_values
            .rows()
            .iter()
            .skip(start_offset)
            .take(args_area.height as usize)
            .enumerate()
            .map(|(i, app_string)| {
                let index = i + start_offset;
                let mut style = Style::default();

                let arg_value = app_string.value();

                let arg = &selected_recipe.arguments()[index];
                let name_prefix = format!("{} = ", arg.name());

                if index == current_index {
                    style = style.bold();

                    if app_state == &AppState::EditingArg {
                        // If in edit mode then show the cursor
                        frame.set_cursor_position(Position::new(
                            args_area.x
                                + app_string.character_index() as u16
                                + name_prefix.len() as u16
                                + 1,
                            args_area.y + i as u16 + 1,
                        ));
                    }
                }
                let content =
                    Line::from(Span::raw(format!("{name_prefix}{arg_value}"))).style(style);
                ListItem::new(content)
            })
            .collect(),
        None => Vec::new(),
    };

    let args = List::new(arg_lines)
        .block(Block::bordered().title("Arguments"))
        .style(block_style(app_state == &AppState::ArgSelection));
    frame.render_widget(args, args_area);
}

pub fn render_ingredients(
    frame: &mut Frame,
    ingredients_area: Rect,
    ingredient_rows: &VecWithIndex<String>,
    app_state: &AppState,
) {
    let middle = ingredients_area.height / 2;
    let current_index = ingredient_rows.index();
    let start_offset = current_index.saturating_sub(middle as usize);

    let lines: Vec<ListItem<'_>> = ingredient_rows
        .rows()
        .iter()
        .skip(start_offset)
        .take(ingredients_area.height as usize)
        .enumerate()
        .map(|(i, command)| {
            let index = i + start_offset;
            let mut style = Style::default();
            if index == current_index {
                style = style.bold();
            }

            let content = Line::from(Span::raw(command.to_string())).style(style);
            ListItem::new(content)
        })
        .collect();

    let ingredients = List::new(lines)
        .block(Block::bordered().title("Ingredients"))
        .style(block_style(app_state == &AppState::IngedientSelection));
    frame.render_widget(ingredients, ingredients_area);
}

pub fn render_messages_area(
    frame: &mut Frame,
    messages_area: Rect,
    messages: &VecWithIndex<String>,
    app_state: &AppState,
) {
    let middle = messages_area.height / 2;
    let current_index = messages.index();
    let start_offset = current_index.saturating_sub(middle as usize);

    let message_lines: Vec<ListItem> = messages
        .rows()
        .iter()
        .skip(start_offset)
        .take(messages_area.height as usize)
        .map(|m| {
            let content = Line::from(Span::raw(m));
            ListItem::new(content)
        })
        .collect();
    let messages = List::new(message_lines)
        .block(Block::bordered().title("Messages"))
        .style(block_style(app_state == &AppState::ViewMessages));
    frame.render_widget(messages, messages_area);
}
