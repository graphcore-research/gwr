// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use ratatui::Frame;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, Paragraph};

use crate::app_string::AppString;
use crate::block_style;
use crate::command::Command;
use crate::vec_with_index::VecWithIndex;
use crate::writer::app::AppState;

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
            "This is a utility for building recipes.",
            "You are presented with the existing history and can select what",
            "entries you want to add to your recipe.",
        ],
    );

    renderer.add_header(
        "Selection:",
        vec!["The tool starts in selection mode where individual lines can be selected/edited."],
    );
    renderer.add_command_help_line("<Esc>", "exit");
    renderer.add_command_help_line("<Tab>", "move to select command editor");
    renderer.add_command_help_line("<Up>,k", "move up in the history");
    renderer.add_command_help_line("<Down>,j", "move down in the history");
    renderer.add_command_help_line("<Space>", "toggle whether the line is in your recipe");
    renderer.add_command_help_line("<Enter>", "edit the selection");
    renderer.add_command_help_line("w", "write out the recipe");
    renderer.add_command_help_line("<PgUp>/<PgDn>", "move up/down a block of history lines");
    renderer.add_command_help_line("ctrl+u/ctrl+f", "same as <PgUp>/<PgDn>");

    renderer.add_header(
        "Select command:",
        vec![
            "You can also use a command to select/deselect/toggle lines in your recipe.",
            "This is of the form:",
            "  [<mode><lines>];+",
            "Where:",
            "  <mode> is one of 's'elect, 'd'e-select, 't'oggle selection",
            "  <lines> is a series of ',' separated lines or line ranges",
            "",
            "For example:",
            "  s10-20,25;d18",
            "will select lines 10-20 and 25 and then de-select line 18.",
            "Note:",
            " '*' can be used to represent all lines",
        ],
    );

    renderer.add_command_help_line("<Enter>", "apply the selection command");
    renderer.add_command_help_line("left-arrow", "move left in command text");
    renderer.add_command_help_line("right-arrow", "move right in command text");
    renderer.add_command_help_line("ctrl+a", "move to start of command text");
    renderer.add_command_help_line("ctrl+e", "move to end of command text");
    renderer.add_command_help_line("ctrl+u", "clear the command text");

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
        AppState::EditingSelectCommand => "Editing Select Command: ".bold(),
        AppState::HistoryEditing => "Editing History Entry: ".bold(),
        AppState::HistorySelection => "Select History Lines: ".bold(),
        AppState::SaveDescription => "Enter Description: ".bold(),
        AppState::SaveFileName => "Enter Filename: ".bold(),
        AppState::ShowHelp => "Help: ".bold(),
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
        AppState::HistorySelection => vec![
            " Move to row, ".into(),
            "Space".bold(),
            " to toggle selection, ".into(),
            "Enter".bold(),
            " to edit and ".into(),
            "w".bold(),
            " to write out a recipe".into(),
        ],
        AppState::HistoryEditing => {
            vec![" Press ".into(), "Esc".bold(), " to stop editing.".into()]
        }
        AppState::EditingSelectCommand => {
            vec![" Enter".bold(), " to apply the selection command.".into()]
        }
        AppState::SaveDescription | AppState::SaveFileName => {
            vec![
                " Press ".into(),
                "Enter".bold(),
                " to save file and ".into(),
                "Esc".bold(),
                " to cancel save.".into(),
            ]
        }
        AppState::ShowHelp => vec![],
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

pub fn render_history(
    frame: &mut Frame,
    history_area: Rect,
    command_history: &VecWithIndex<Command>,
    app_state: &AppState,
) {
    let middle = history_area.height / 2;
    let history_row_index = command_history.index();
    let start_offset = history_row_index.saturating_sub(middle as usize);

    let history_lines: Vec<ListItem> = command_history
        .rows()
        .iter()
        .skip(start_offset)
        .take(history_area.height as usize)
        .enumerate()
        .map(|(i, command)| {
            let line_index = i + start_offset;
            let index_prefix = format!("{line_index}: ");
            let app_string = command.app_string();

            // Show whether the line is currently selected by changing the formatting
            let mut style = if command.selected() {
                Style::default().bg(Color::LightBlue).fg(Color::White)
            } else {
                Style::default()
            };

            if line_index == history_row_index {
                // Show the current line in bold
                style = style.bold();

                if app_state == &AppState::HistoryEditing {
                    // If in edit mode then show the cursor
                    frame.set_cursor_position(Position::new(
                        history_area.x
                            + app_string.character_index() as u16
                            + index_prefix.len() as u16
                            + 1,
                        history_area.y + i as u16 + 1,
                    ));
                }
            }

            let content = Line::from(Span::raw(format!("{index_prefix}{command}"))).style(style);
            ListItem::new(content)
        })
        .collect();

    let history = List::new(history_lines)
        .block(Block::bordered().title("History"))
        .style(block_style(app_state == &AppState::HistorySelection));
    frame.render_widget(history, history_area);
}

pub fn render_message_area(frame: &mut Frame, messages_area: Rect, message: &str) {
    let message_lines: Vec<ListItem> = message
        .lines()
        .map(|m| {
            let content = Line::from(Span::raw(m));
            ListItem::new(content)
        })
        .collect();
    let messages = List::new(message_lines).block(Block::bordered().title("Messages"));
    frame.render_widget(messages, messages_area);
}
