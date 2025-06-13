// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cmp::max;
use std::vec;

use tui::Frame;
use tui::backend::Backend;
use tui::layout::Direction;
use tui::layout::Layout;
use tui::layout::{Alignment, Constraint, Rect};
use tui::style::Modifier;
use tui::style::{Color, Style};
use tui::text::{Line, Span};
use tui::widgets::BarChart;
use tui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::App;
use crate::app::InputState;
use crate::handler::TOGGLE_RE;

/// Renders the user interface widgets.
pub fn render<B: Backend>(app: &mut App, frame: &mut Frame<'_, B>) {
    // This is where you add new widgets.
    // See the following resources:
    // - https://docs.rs/ratatui/latest/ratatui/widgets/index.html
    // - https://github.com/ratatui-org/ratatui/tree/master/examples

    // Default is to use the entire area to render the log.
    let mut log_area = frame.size();

    if app.state() == InputState::Help {
        render_help(app, frame, log_area);
        return;
    }

    if app.state() == InputState::Search {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Max(4)].as_ref())
            .split(log_area);
        log_area = chunks[0];
        let search_area = chunks[1];

        render_search(app, frame, search_area);
    }

    if app.filter.lock().unwrap().tag_defined() {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
            .split(log_area);
        log_area = chunks[0];
        let chart_area = chunks[1];
        render_chart(app, frame, chart_area);
    }

    render_log(app, frame, log_area);
}

fn get_search_and_cursor(app: &App) -> (String, usize) {
    let guard = app.filter.lock().unwrap();
    let mut search = guard.search.to_owned();
    // Add extra space to highlight as cursor if at end of line.
    search.push(' ');
    (search, guard.search_cursor_pos)
}

fn render_search<B: Backend>(app: &mut App, frame: &mut Frame<'_, B>, area: Rect) {
    let title = "Search";
    let re = format!("Regex ({TOGGLE_RE:?})")
        .replace('(', "")
        .replace(')', "");

    let (search, cursor_pos) = get_search_and_cursor(app);

    let before = &search[..cursor_pos];
    let cursor = &search[cursor_pos..cursor_pos + 1];
    let after = &search[cursor_pos + 1..];

    let mut text = vec![Line::from(vec![
        Span::from(before),
        Span::styled(cursor, Style::default().bg(Color::Red).fg(Color::Black)),
        Span::from(after),
    ])];

    if app.filter.lock().unwrap().use_regex {
        text.push(Line::from(vec![Span::styled(
            re,
            Style::default().bg(Color::Blue).fg(Color::White),
        )]));
    } else {
        text.push(Line::from(vec![Span::from(re)]));
    }

    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .title(title)
                    .title_alignment(Alignment::Left)
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .style(Style::default().fg(Color::Cyan).bg(Color::Black))
            .alignment(Alignment::Left),
        area,
    );
}

fn render_log<B: Backend>(app: &mut App, frame: &mut Frame<'_, B>, area: Rect) {
    // Update the renderer with the current frame size.
    app.set_frame_size(area.height as usize);

    let renderer = app.renderer.lock().unwrap();
    let mut text = String::new();
    for index in renderer.into_iter().take(area.height as usize) {
        text.push_str(renderer.render_line(index).as_str());
        text.push('\n');
    }
    let pos = format!(
        "{}/{}/{}",
        renderer.current_render_line_number(),
        renderer.num_render_lines,
        renderer.num_lines
    )
    .to_owned();

    let title = match app.state() {
        InputState::Goto => "g".to_owned(),
        InputState::Numbers => app.numbers.to_owned(),
        _ => pos,
    };

    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .title(title)
                    .title_alignment(Alignment::Left)
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .style(Style::default().fg(Color::Cyan).bg(Color::Black))
            .alignment(Alignment::Left),
        area,
    );
}

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

    fn add_sub_command_help_line(&mut self, command: &'a str, help: &'a str) {
        self.lines.push(Line::from(vec![
            Span::from(self.indent),
            Span::styled(" - ", self.style_text),
            Span::styled(command, self.style_command),
            Span::styled(format!(": {help}"), self.style_text),
        ]));
    }

    fn add_blank_line(&mut self) {
        self.lines.push(Line::from(vec![Span::from("")]));
    }
}

fn render_help<B: Backend>(_app: &mut App, frame: &mut Frame<'_, B>, area: Rect) {
    let mut renderer = HelpRender::new();

    let re = format!("{TOGGLE_RE:?}").replace('(', "").replace(')', "");
    renderer.add_header(
        "Search:",
        vec![
            "Enter a search/filter text string.",
            "Can contain 'tag=<NUMBER>' to filter down to one unique tag",
        ],
    );
    renderer.add_command_help_line("/", "enable search window");
    renderer.add_command_help_line("ctrl+u", "clear from cursor to start of search line");
    renderer.add_command_help_line("left-arrow", "move left in search text");
    renderer.add_command_help_line("right-arrow", "move right in search text");
    renderer.add_command_help_line("ctrl+a", "move to start of search text");
    renderer.add_command_help_line("ctrl+e", "move to end of search text");
    renderer.add_command_help_line("ctrl+e", "move to end of search text");
    renderer.add_command_help_line(re.as_str(), "toggle regular-expression mode");

    renderer.add_header("Navigation:", vec![]);
    renderer.add_command_help_line("up/down-arrow", "move up/down a single line");
    renderer.add_command_help_line("PgUp/PgDn", "move up/down a block of lines");
    renderer.add_command_help_line("ctrl+u/ctrl+f", "same as PgUp/PgDn");

    renderer.add_blank_line();
    renderer.add_command_help_line("[0-9]", "type a number followed by:");
    renderer.add_sub_command_help_line("%", "goto to that percent position in file");
    renderer.add_sub_command_help_line("G", "goto to that line number in file");
    renderer.add_sub_command_help_line("Enter", "goto down that many lines");

    renderer.add_blank_line();
    renderer.add_command_help_line("g", "followed by:");
    renderer.add_sub_command_help_line("g", "goto to start of the file");
    renderer.add_sub_command_help_line("G", "goto to end of the file");

    renderer.add_header("Display:", vec![]);
    renderer.add_command_help_line("n", "toggle show names");
    renderer.add_command_help_line("p", "toggle show packet contents");
    renderer.add_command_help_line("f", "toggle plot fullness / times");

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

fn render_chart<B: Backend>(app: &mut App, frame: &mut Frame<'_, B>, area: Rect) {
    let renderer = app.renderer.lock().unwrap();
    let plot_fullness = renderer.plot_fullness;
    let mut data = Vec::new();
    for index in renderer.into_iter().take(area.height as usize) {
        let value = if plot_fullness {
            renderer.line_fullness(index)
        } else {
            renderer.line_time(index) as u64
        };
        data.push(("", value));
    }

    let mut max_value = u64::MIN;
    if let Some(indices) = renderer.render_indices.as_ref() {
        for index in indices {
            let value = if plot_fullness {
                renderer.line_fullness(*index)
            } else {
                renderer.line_time(*index) as u64
            };
            max_value = max(max_value, value);
        }
    }

    let title = if plot_fullness { "Fullness:" } else { "Time:" };

    let barchart = BarChart::default()
        .block(Block::default().title(title).borders(Borders::ALL))
        .data(&data)
        .max(max_value)
        .bar_width(1)
        .bar_gap(0)
        .group_gap(0)
        .bar_style(Style::default().fg(Color::Cyan).bg(Color::Black))
        .value_style(Style::default().fg(Color::White).bg(Color::Red))
        .style(Style::default().fg(Color::Cyan).bg(Color::Black))
        .direction(Direction::Horizontal);

    frame.render_widget(barchart, area);
}
