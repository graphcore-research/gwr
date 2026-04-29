// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, Paragraph, Wrap};
use ratatui::{Frame, symbols};

use crate::tui::app::{App, PlotConfig};

const PLOT_TIME_LABEL_COUNT: usize = 8;

pub fn render(app: &mut App, frame: &mut Frame) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(frame.area());

    render_header(app, frame, vertical[0]);
    render_body(app, frame, vertical[1]);
    render_footer(frame, vertical[2]);
}

fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    let title = format!(
        "Staff {} | Tick {} / {} | Frame {} / {} | {} | Window {} ticks",
        app.recording.staffing,
        app.current_tick(),
        app.recording.summary.finish_tick,
        app.frame_index + 1,
        app.recording.timeline.len(),
        if app.playing {
            format!("playing {}", app.speed_label())
        } else {
            format!("paused {}", app.speed_label())
        },
        app.window_size_ticks()
    );

    let summary = &app.recording.summary;
    let lines = vec![
        Line::from(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "Arrivals {} | Served {} | Balked {} | GaveUp {} | Abandoned {} | Profit {:.2}",
            summary.arrivals,
            summary.served,
            summary.balked,
            summary.gave_up_queue,
            summary.abandoned,
            summary.profit
        )),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("Playback"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_body(app: &App, frame: &mut Frame, area: Rect) {
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
        .split(area);

    render_state_panels(app, frame, horizontal[0]);
    render_plots(app, frame, horizontal[1]);
}

fn render_state_panels(app: &App, frame: &mut Frame, area: Rect) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Min(8),
        ])
        .split(area);

    render_system_state(app, frame, sections[0]);
    render_customer_states(app, frame, sections[1]);
    render_recent_events(app, frame, sections[2]);
}

fn render_system_state(app: &App, frame: &mut Frame, area: Rect) {
    let snapshot = app.current_snapshot();
    let metrics = &snapshot.metrics;
    let staffing_cost = accrued_staffing_cost(app);
    let overview = vec![
        formatted_line(
            vec![
                ("Till queue", format!("{:>3}", snapshot.till_queue.len())),
                (
                    "Kitchen queue",
                    format!("{:>3}", snapshot.kitchen_queue.len()),
                ),
            ],
            "    ",
        ),
        formatted_line(
            vec![
                ("Busy till", format!("{:>3}", snapshot.active_till_workers)),
                (
                    "Busy kitchen",
                    format!("{:>3}", snapshot.active_kitchen_workers),
                ),
            ],
            "     ",
        ),
        formatted_line(
            vec![
                ("Revenue", format!("{:>8.2}", metrics.revenue)),
                (
                    "Ingredient cost",
                    format!("{:>8.2}", metrics.ingredient_cost),
                ),
            ],
            "   ",
        ),
        formatted_line(
            vec![("Staffing cost", format!("{staffing_cost:>8.2}"))],
            "   ",
        ),
        formatted_line(
            vec![
                ("Orders started", format!("{:>3}", metrics.orders_started)),
                ("served", format!("{:>3}", metrics.orders_served)),
                ("abandoned", format!("{:>3}", metrics.orders_abandoned)),
            ],
            "   ",
        ),
        formatted_line(
            vec![
                ("Closed", snapshot.closed.to_string()),
                ("Arrivals complete", snapshot.arrivals_complete.to_string()),
            ],
            "   ",
        ),
    ];

    frame.render_widget(
        Paragraph::new(overview)
            .block(Block::default().borders(Borders::ALL).title("System State"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_customer_states(app: &App, frame: &mut Frame, area: Rect) {
    let snapshot = app.current_snapshot();
    let counts = &snapshot.customer_counts;
    let customer_lines = vec![
        formatted_line(
            vec![
                ("Planned", format!("{:>3}", counts.planned)),
                ("Balked", format!("{:>3}", counts.balked)),
            ],
            "    ",
        ),
        formatted_line(
            vec![
                ("Waiting till", format!("{:>3}", counts.waiting_till)),
                ("At till", format!("{:>3}", counts.at_till)),
            ],
            "   ",
        ),
        formatted_line(
            vec![
                ("Waiting kitchen", format!("{:>3}", counts.waiting_kitchen)),
                ("Preparing", format!("{:>3}", counts.preparing_food)),
            ],
            "   ",
        ),
        formatted_line(
            vec![
                ("Collecting", format!("{:>3}", counts.collecting_food)),
                ("Served", format!("{:>3}", counts.served)),
            ],
            "   ",
        ),
        formatted_line(
            vec![
                ("Gave up", format!("{:>3}", counts.gave_up_queue)),
                ("Abandoned", format!("{:>3}", counts.abandoned)),
            ],
            "   ",
        ),
    ];
    frame.render_widget(
        Paragraph::new(customer_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Customer States"),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_recent_events(app: &App, frame: &mut Frame, area: Rect) {
    let visible_event_lines = usize::from(area.height.saturating_sub(2)).max(1);
    let mut event_lines = Vec::new();
    for event in app.recent_events(visible_event_lines).into_iter().rev() {
        event_lines.push(Line::from(format!("[{:>5}] {}", event.tick, event.message)));
    }
    if event_lines.is_empty() {
        event_lines.push(Line::from("No events yet"));
    }
    frame.render_widget(
        Paragraph::new(event_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Recent Events"),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_plots(app: &App, frame: &mut Frame, area: Rect) {
    let plot_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);

    for (index, plot_area) in plot_areas.iter().enumerate() {
        render_plot(app, frame, *plot_area, index, app.plots[index]);
    }
}

fn render_plot(app: &App, frame: &mut Frame, area: Rect, index: usize, plot: PlotConfig) {
    let salary_cost = app.recording.summary.salary_cost;
    let day_ticks = app.recording.day_ticks;
    let window_ticks = app.window_size_ticks();
    let data: Vec<(f64, f64)> = app
        .recording
        .timeline
        .iter()
        .enumerate()
        .take(app.frame_index + 1)
        .map(|(point_index, point)| {
            (
                point.tick as f64,
                plot_value(
                    &app.recording.timeline,
                    point_index,
                    plot,
                    day_ticks,
                    salary_cost,
                    window_ticks,
                ),
            )
        })
        .collect();

    let max_x = app.recording.summary.finish_tick.max(1) as f64;
    let min_y = data
        .iter()
        .map(|(_, y)| *y)
        .fold(0.0_f64, f64::min)
        .min(0.0);
    let max_y = data
        .iter()
        .map(|(_, y)| *y)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let title = format!(
        "Plot {}: {}{}",
        index + 1,
        plot.stat.title(plot.windowed, window_ticks),
        if app.selected_plot == index {
            " <selected>"
        } else {
            ""
        }
    );

    let dataset = Dataset::default()
        .marker(symbols::Marker::Braille)
        .style(Style::default().fg(match index {
            0 => Color::Cyan,
            1 => Color::Yellow,
            _ => Color::Green,
        }))
        .graph_type(ratatui::widgets::GraphType::Line)
        .data(&data);

    let mut datasets = Vec::new();
    let zero_line = [(0.0, 0.0), (max_x, 0.0)];
    if min_y < 0.0 && max_y > 0.0 {
        datasets.push(
            Dataset::default()
                .style(Style::default().fg(Color::DarkGray))
                .graph_type(ratatui::widgets::GraphType::Line)
                .data(&zero_line),
        );
    }
    datasets.push(dataset);

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(if app.selected_plot == index {
                    Style::default().fg(Color::Blue)
                } else {
                    Style::default()
                }),
        )
        .x_axis(
            Axis::default()
                .bounds([0.0, max_x])
                .labels(plot_time_labels(app, max_x)),
        )
        .y_axis(Axis::default().bounds([min_y, max_y]).labels(vec![
            Span::raw(format!("{min_y:.1}")),
            Span::raw(format!("{:.1}", f64::midpoint(min_y, max_y))),
            Span::raw(format!("{max_y:.1}")),
        ]));

    frame.render_widget(chart, area);
}

fn plot_time_labels(app: &App, max_x: f64) -> Vec<Span<'static>> {
    if PLOT_TIME_LABEL_COUNT <= 2 {
        return vec![time_label(app, 0), time_label(app, max_x.round() as u64)];
    }

    (0..PLOT_TIME_LABEL_COUNT)
        .map(|index| {
            let position = max_x * index as f64 / (PLOT_TIME_LABEL_COUNT - 1) as f64;

            time_label(app, position.round() as u64)
        })
        .collect()
}

fn time_label(app: &App, offset_ticks: u64) -> Span<'static> {
    Span::raw(
        app.recording
            .opening_time
            .add_ticks(offset_ticks)
            .round_to_nearest_quarter_hour()
            .to_string(),
    )
}

fn render_footer(frame: &mut Frame, area: Rect) {
    let key_style = Style::default().add_modifier(Modifier::BOLD);
    let help = Line::from(vec![
        Span::styled("q", key_style),
        Span::raw(" quit | "),
        Span::styled("space", key_style),
        Span::raw(" play/pause | "),
        Span::styled("g/G", key_style),
        Span::raw(" start/end | "),
        Span::styled("h/l", key_style),
        Span::raw(" or "),
        Span::styled("left/right", key_style),
        Span::raw(" step | "),
        Span::styled("[ ]", key_style),
        Span::raw(" speed | "),
        Span::styled("{ }", key_style),
        Span::raw(" window | "),
        Span::styled("w", key_style),
        Span::raw(" toggle windowed | "),
        Span::styled("1-4", key_style),
        Span::raw(" select plot | "),
        Span::styled("j/k", key_style),
        Span::raw(" or "),
        Span::styled("up/down", key_style),
        Span::raw(" change stat"),
    ]);
    frame.render_widget(
        Paragraph::new(help).block(Block::default().borders(Borders::ALL).title("Controls")),
        area,
    );
}

fn formatted_line(items: Vec<(&str, String)>, separator: &str) -> Line<'static> {
    let label_style = Style::default().add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();

    for (index, (label, value)) in items.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(separator.to_string()));
        }
        spans.push(Span::styled(format!("{label}:"), label_style));
        spans.push(Span::raw(format!(" {value}")));
    }

    Line::from(spans)
}

fn plot_value(
    timeline: &[crate::recording::TimelinePoint],
    point_index: usize,
    plot: PlotConfig,
    day_ticks: u64,
    salary_cost: f64,
    window_ticks: u64,
) -> f64 {
    let point = &timeline[point_index];
    let current = plot
        .stat
        .base_value(&point.snapshot, point.tick, salary_cost, day_ticks);
    if !(plot.windowed && plot.stat.supports_windowing()) {
        return current;
    }

    let window_start_tick = point.tick.saturating_sub(window_ticks);
    let base_index = timeline[..=point_index]
        .iter()
        .rposition(|timeline_point| timeline_point.tick <= window_start_tick);

    match base_index {
        Some(index) => {
            let prior = plot.stat.base_value(
                &timeline[index].snapshot,
                timeline[index].tick,
                salary_cost,
                day_ticks,
            );
            current - prior
        }
        None => current,
    }
}

fn accrued_staffing_cost(app: &App) -> f64 {
    let total_salary_cost = app.recording.summary.salary_cost;
    let day_ticks = app.recording.day_ticks;
    if day_ticks == 0 {
        return 0.0;
    }

    total_salary_cost * (app.current_tick().min(day_ticks) as f64 / day_ticks as f64)
}
