// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::fmt::Write as _;

use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Document, HtmlElement};

use super::super::format::{bytes, bytes_u64, count, decimal, escape, integer, label};
use super::super::logic::AppModel;
use super::{compute_nodes_markup, element, machine_ops_markup, set_html};
use crate::model::{PeSummary, TensorSummary};

struct Measure<'a> {
    group: &'static str,
    label: String,
    colour: &'static str,
    unit: &'a str,
    context: Option<&'a str>,
}

struct ValueRange {
    minimum: f64,
    maximum: f64,
    observed_minimum: f64,
    observed_maximum: f64,
    span: f64,
    magnitude: f64,
    zero_percent: f64,
}

struct GridPeContext<'model, 'render> {
    model: &'model AppModel,
    measure: &'render Measure<'model>,
    range: &'render ValueRange,
}

pub(super) fn render_overview(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    set_overview_mode(model, document)?;
    if model.state.pe_mode == "chart" {
        render_chart(model, document)
    } else {
        render_grid(model, document)
    }
}

pub(super) fn render_detail(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let Some(pe) = model.selected_pe() else {
        return set_html(document, "selected-pe", "No processing elements found.");
    };
    let population = model.compute_population();
    let values = population
        .iter()
        .map(|candidate| model.machine_ops_for_pe(candidate).total as f64)
        .collect::<Vec<_>>();
    let selected_value = model.machine_ops_for_pe(pe).total as f64;
    let maximum = values.iter().copied().fold(1.0_f64, f64::max);
    let average = if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    };
    let selected_traffic = model.context(None, Some(&pe.name));
    let traffic_maximum = population
        .iter()
        .map(|candidate| model.context(None, Some(&candidate.name)))
        .flat_map(|traffic| [traffic.read_bytes, traffic.write_bytes])
        .fold(1_u64, u64::max);
    let (nodes, by_op) = model.compute_nodes_for_pe(&pe.name);
    let platform = platform_markup(pe);
    let overlays = overlay_markup(model, pe);
    let html = format!(
        "<h2>{}</h2><p>Row {}, column {}</p>{platform}<div class=\"selected-compute\" aria-label=\"Static compute allocation\"><div><span>Machine ops</span><strong>{}</strong></div><div class=\"selected-compute-track\"><div style=\"width: {}%\"></div><i style=\"left: {}%\" aria-hidden=\"true\"></i></div><p>{}% of maximum · average {}</p>{}{}</div><div class=\"pe-traffic\" aria-label=\"Tensor traffic\"><span>Read</span><div class=\"traffic-track read\"><div style=\"width: {}%\"></div></div><strong>{} <em>{}%</em></strong><span>Written</span><div class=\"traffic-track write\"><div style=\"width: {}%\"></div></div><strong>{} <em>{}%</em></strong></div><div class=\"overlay-list\">{overlays}</div>",
        escape(&pe.name),
        pe.row,
        pe.col,
        count(selected_value),
        selected_value / maximum * 100.0,
        average / maximum * 100.0,
        decimal(selected_value / maximum * 100.0, 1),
        count(average),
        compute_nodes_markup(model, nodes, &by_op),
        machine_ops_markup(model, &model.machine_ops_for_pe(pe)),
        selected_traffic.read_bytes as f64 / traffic_maximum as f64 * 100.0,
        bytes_u64(selected_traffic.read_bytes),
        decimal(
            selected_traffic.read_bytes as f64 / traffic_maximum as f64 * 100.0,
            1,
        ),
        selected_traffic.write_bytes as f64 / traffic_maximum as f64 * 100.0,
        bytes_u64(selected_traffic.write_bytes),
        decimal(
            selected_traffic.write_bytes as f64 / traffic_maximum as f64 * 100.0,
            1,
        ),
    );
    set_html(document, "selected-pe", &html)
}

fn render_chart(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let population = model.compute_population();
    let measure = measure(model);
    let values = measure_values(model);
    let mut rows = population
        .into_iter()
        .filter_map(|pe| values.get(&pe.name).map(|value| (pe, *value)))
        .collect::<Vec<_>>();
    rows.sort_by(|(left_pe, left), (right_pe, right)| {
        right
            .total_cmp(left)
            .then_with(|| left_pe.row.cmp(&right_pe.row))
            .then_with(|| left_pe.col.cmp(&right_pe.col))
    });
    let range = value_range(rows.iter().map(|(_, value)| *value));
    let average = if rows.is_empty() {
        0.0
    } else {
        rows.iter().map(|(_, value)| value).sum::<f64>() / rows.len() as f64
    };
    let context = measure
        .context
        .map(|value| format!(" · {}", escape(value)))
        .unwrap_or_default();
    let has_rows = !rows.is_empty();
    let mut html = if has_rows {
        format!(
            "<div class=\"pe-overview-chart-legend\"><span>{} · {}{context}</span><span>Minimum {}</span><span>Average {}</span><span>Maximum {}</span></div><div class=\"pe-overview-chart-list\">",
            escape(measure.group),
            escape(&measure.label),
            format_value(range.observed_minimum, &measure),
            format_value(average, &measure),
            format_value(range.observed_maximum, &measure),
        )
    } else {
        format!(
            "<div class=\"pe-overview-chart-legend\"><span>{} · {}{context}</span><span>No values supplied</span></div>",
            escape(measure.group),
            escape(&measure.label),
        )
    };
    for (pe, value) in rows {
        let start = value.min(0.0);
        let left = (start - range.minimum) / range.span * 100.0;
        let width = value.abs() / range.span * 100.0;
        let selected = model.state.selected_pe.as_deref() == Some(pe.name.as_str());
        write!(
            html,
            "<button type=\"button\" class=\"pe-overview-chart-row{}\" data-select-kind=\"pe\" data-select-id=\"{}\" data-selection-kind=\"pe\" data-selection-id=\"{}\" aria-pressed=\"{}\" aria-label=\"{}, {} {} {}\"><span>{}</span><div class=\"pe-overview-chart-track\"><div class=\"pe-overview-chart-fill{}\" style=\"left: {left}%; width: {width}%\"></div><i style=\"left: {}%\" aria-hidden=\"true\"></i></div><strong>{}</strong></button>",
            selected.then_some(" selected").unwrap_or(""),
            escape(&pe.name),
            escape(&pe.name),
            selected,
            escape(&pe.name),
            format_value(value, &measure),
            escape(measure.group),
            escape(&measure.label),
            escape(&pe.name),
            (value < 0.0).then_some(" negative").unwrap_or(""),
            (average - range.minimum) / range.span * 100.0,
            format_value(value, &measure),
        )
        .unwrap();
    }
    if has_rows {
        html.push_str("</div>");
    }
    set_html(document, "pe-overview-chart", &html)?;
    style(document, "pe-overview-chart")?
        .set_property("--overview-colour", &format!("var({})", measure.colour))
}

fn render_grid(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let (rows, cols) = model.dimensions();
    let measure = measure(model);
    let values = measure_values(model);
    let population = model.compute_population();
    let range = value_range(
        population
            .iter()
            .filter_map(|pe| values.get(&pe.name).copied()),
    );
    let nodes = compute_node_values(model);
    let context = GridPeContext {
        model,
        measure: &measure,
        range: &range,
    };
    let mut by_coordinate: HashMap<(usize, usize), Vec<&PeSummary>> = HashMap::new();
    for pe in &model.data.pes {
        by_coordinate.entry((pe.row, pe.col)).or_default().push(pe);
    }
    let mut html = String::new();
    for row in 0..rows {
        for col in 0..cols {
            let candidates = by_coordinate
                .get(&(row, col))
                .map(Vec::as_slice)
                .unwrap_or_default();
            let columns = (candidates.len() as f64).sqrt().ceil().max(1.0) as usize;
            write!(
                html,
                "<div class=\"pe-cell{}\"{}>",
                (candidates.len() > 1).then_some(" multiple").unwrap_or(""),
                (candidates.len() > 1)
                    .then(|| format!(
                        " style=\"grid-template-columns: repeat({columns}, minmax(0, 1fr))\""
                    ))
                    .unwrap_or_default(),
            )
            .unwrap();
            if candidates.is_empty() {
                write!(html, "<button type=\"button\" class=\"pe empty\" disabled aria-label=\"No processing element at {row}, {col}\"></button>").unwrap();
            }
            for pe in candidates {
                render_grid_pe(
                    &mut html,
                    pe,
                    values.get(&pe.name).copied(),
                    nodes.get(&pe.name).copied().unwrap_or(0.0),
                    &context,
                );
            }
            html.push_str("</div>");
        }
    }
    set_html(document, "pe-grid", &html)?;
    set_axes(document, rows, cols)?;
    set_grid_dimensions(document, rows, cols)?;
    render_legend(model, document, &measure, &values, &range)
}

fn render_grid_pe(
    html: &mut String,
    pe: &PeSummary,
    value: Option<f64>,
    compute_nodes: f64,
    context: &GridPeContext<'_, '_>,
) {
    let matches = context.model.state.pes.is_selected(&pe.name);
    let normalized = value.map_or(0.0, |value| value.abs() / context.range.magnitude);
    let intensity = if matches && value.is_some_and(|value| value != 0.0) {
        (10.0 + normalized.sqrt() * 90.0).round()
    } else {
        0.0
    };
    let selected = context.model.state.selected_pe.as_deref() == Some(pe.name.as_str());
    let class = format!(
        "pe{}{}{}",
        value.is_none().then_some(" unavailable").unwrap_or(""),
        selected.then_some(" selected").unwrap_or(""),
        (!matches).then_some(" filtered-out").unwrap_or(""),
    );
    let formatted = value.map_or_else(
        || "no value supplied".into(),
        |value| format_value(value, context.measure),
    );
    let colour = if value.is_some_and(|value| value < 0.0) {
        "--write"
    } else {
        context.measure.colour
    };
    write!(
        html,
        "<button type=\"button\" class=\"{class}\" title=\"{}\" style=\"--grid-colour: var({colour}); --intensity: {intensity}%; --platform: {}%\" data-select-kind=\"pe\" data-select-id=\"{}\" data-selection-kind=\"pe\" data-selection-id=\"{}\" aria-pressed=\"{}\" aria-label=\"{}, {formatted} {} {}, {} compute nodes\"></button>",
        escape(if value.is_some() { &pe.name } else { "No value supplied" }),
        if pe.present_in_platform { 16 } else { 0 },
        escape(&pe.name),
        escape(&pe.name),
        selected,
        escape(&pe.name),
        escape(context.measure.group),
        escape(&context.measure.label),
        integer(compute_nodes.round().max(0.0) as u64),
    )
    .unwrap();
}

fn render_legend(
    model: &AppModel,
    document: &Document,
    measure: &Measure<'_>,
    values: &HashMap<String, f64>,
    range: &ValueRange,
) -> Result<(), JsValue> {
    let population = model.compute_population();
    let observed = population
        .iter()
        .filter_map(|pe| values.get(&pe.name))
        .copied()
        .collect::<Vec<_>>();
    let context = measure
        .context
        .map(|value| format!("<em>{}</em>", escape(value)))
        .unwrap_or_default();
    let stats = if observed.is_empty() {
        "<div class=\"pe-overview-legend-stats\"><span>No values supplied</span></div>".into()
    } else {
        let average = observed.iter().sum::<f64>() / observed.len() as f64;
        format!(
            "<div class=\"pe-overview-legend-stats\"><span>Minimum {}</span><span>Average {}</span><span>Maximum {}</span></div><div class=\"pe-overview-legend-scale\" aria-hidden=\"true\"><span>{}</span><i class=\"{}\"></i><span>{}</span></div>",
            format_value(range.observed_minimum, measure),
            format_value(average, measure),
            format_value(range.observed_maximum, measure),
            format_value(range.minimum, measure),
            if range.minimum < 0.0 && range.maximum > 0.0 {
                "signed"
            } else if range.maximum <= 0.0 {
                "negative"
            } else {
                ""
            },
            format_value(range.maximum, measure),
        )
    };
    set_html(
        document,
        "pe-overview-legend",
        &format!(
            "<div class=\"pe-overview-legend-title\"><span>{}</span><strong>{}</strong>{context}</div>{stats}",
            escape(measure.group),
            escape(&measure.label),
        ),
    )?;
    let legend = style(document, "pe-overview-legend")?;
    legend.set_property("--grid-colour", &format!("var({})", measure.colour))?;
    legend.set_property("--zero-position", &format!("{}%", range.zero_percent))
}

fn measure_values(model: &AppModel) -> HashMap<String, f64> {
    let value = model.state.pe_measure.as_str();
    if value == "compute:compute-nodes" {
        return compute_node_values(model);
    }
    if let Some(operation) = value.strip_prefix("compute:machine-op:") {
        return model
            .data
            .pes
            .iter()
            .map(|pe| {
                let operations = model.machine_ops_for_pe(pe);
                let value = match operation {
                    "adds" => operations.adds,
                    "muls" => operations.muls,
                    "compares" => operations.compares,
                    _ => 0,
                };
                (pe.name.clone(), value as f64)
            })
            .collect();
    }
    if value == "compute:machine-ops" {
        return model
            .data
            .pes
            .iter()
            .map(|pe| (pe.name.clone(), model.machine_ops_for_pe(pe).total as f64))
            .collect();
    }
    if let Some(direction) = value.strip_prefix("data:") {
        return traffic_values(model, model.filtered_tensors(), direction);
    }
    if let Some(direction) = value.strip_prefix("tensor:") {
        let tensors = model.selected_tensor().into_iter().collect::<Vec<_>>();
        return traffic_values(model, tensors, direction);
    }
    if let Some(name) = value.strip_prefix("metric:") {
        return model
            .data
            .pes
            .iter()
            .filter_map(|pe| pe.overlays.get(name).map(|value| (pe.name.clone(), *value)))
            .collect();
    }
    HashMap::new()
}

fn compute_node_values(model: &AppModel) -> HashMap<String, f64> {
    model
        .data
        .pes
        .iter()
        .map(|pe| {
            (
                pe.name.clone(),
                model.compute_nodes_for_pe(&pe.name).0 as f64,
            )
        })
        .collect()
}

fn traffic_values(
    model: &AppModel,
    tensors: Vec<&TensorSummary>,
    direction: &str,
) -> HashMap<String, f64> {
    let mut values = HashMap::new();
    for tensor in tensors {
        let traffic = model.tensor_traffic(tensor);
        if direction == "read" || direction == "total" {
            add_connections(&mut values, &traffic.reads);
        }
        if direction == "write" || direction == "total" {
            add_connections(&mut values, &traffic.writes);
        }
    }
    values
}

fn add_connections(
    values: &mut HashMap<String, f64>,
    connections: &[super::super::logic::VisibleConnection],
) {
    for connection in connections {
        *values.entry(connection.pe.clone()).or_default() += connection.bytes as f64;
    }
}

fn measure(model: &AppModel) -> Measure<'_> {
    let value = model.state.pe_measure.as_str();
    let selected_tensor = model.state.selected_tensor.as_deref();
    match value {
        "compute:machine-ops" => Measure {
            group: "Compute allocation",
            label: "Machine ops".into(),
            colour: "--activity",
            unit: "count",
            context: None,
        },
        "compute:compute-nodes" => Measure {
            group: "Compute allocation",
            label: "Compute nodes".into(),
            colour: "--activity",
            unit: "count",
            context: None,
        },
        "data:total" => Measure {
            group: "Data",
            label: "Total".into(),
            colour: "--activity",
            unit: "bytes",
            context: None,
        },
        "data:read" => Measure {
            group: "Data",
            label: "Read".into(),
            colour: "--read",
            unit: "bytes",
            context: None,
        },
        "data:write" => Measure {
            group: "Data",
            label: "Written".into(),
            colour: "--write",
            unit: "bytes",
            context: None,
        },
        "tensor:read" => Measure {
            group: "Selected tensor",
            label: "Read bytes".into(),
            colour: "--read",
            unit: "bytes",
            context: selected_tensor,
        },
        "tensor:write" => Measure {
            group: "Selected tensor",
            label: "Written bytes".into(),
            colour: "--write",
            unit: "bytes",
            context: selected_tensor,
        },
        _ => custom_measure(model, value),
    }
}

fn custom_measure<'a>(model: &'a AppModel, value: &str) -> Measure<'a> {
    if let Some(operation) = value.strip_prefix("compute:machine-op:") {
        let operation_label = model
            .data
            .machine_ops
            .iter()
            .find(|candidate| candidate.name == operation)
            .map_or_else(|| label(operation), |candidate| candidate.label.clone());
        return Measure {
            group: "Compute allocation",
            label: operation_label,
            colour: "--activity",
            unit: "count",
            context: None,
        };
    }
    let name = value.strip_prefix("metric:").unwrap_or(value);
    let metadata = model.data.overlay_metrics.get(name);
    Measure {
        group: "Metrics file",
        label: metadata
            .and_then(|value| value.label.clone())
            .unwrap_or_else(|| label(name)),
        colour: "--metric",
        unit: metadata
            .and_then(|value| value.unit.as_deref())
            .unwrap_or(""),
        context: None,
    }
}

fn format_value(value: f64, measure: &Measure<'_>) -> String {
    match measure.unit {
        "count" => count(value),
        "bytes" => bytes(value),
        "ns" => duration(value),
        "%" => format!("{}%", decimal(value, 2)),
        "" => decimal(value, 2),
        unit => format!("{} {}", decimal(value, 2), escape(unit)),
    }
}

fn duration(nanoseconds: f64) -> String {
    for (scale, unit) in [(1e9, "s"), (1e6, "ms"), (1e3, "us")] {
        if nanoseconds.abs() >= scale {
            return format!("{} {unit}", decimal(nanoseconds / scale, 2));
        }
    }
    format!("{} ns", decimal(nanoseconds, 2))
}

fn value_range(values: impl Iterator<Item = f64>) -> ValueRange {
    let values = values.collect::<Vec<_>>();
    let observed_minimum = values.iter().copied().reduce(f64::min).unwrap_or(0.0);
    let observed_maximum = values.iter().copied().reduce(f64::max).unwrap_or(0.0);
    let minimum = observed_minimum.min(0.0);
    let maximum = observed_maximum.max(0.0);
    let difference = maximum - minimum;
    ValueRange {
        minimum,
        maximum,
        observed_minimum,
        observed_maximum,
        span: if difference == 0.0 { 1.0 } else { difference },
        magnitude: minimum.abs().max(maximum.abs()).max(1.0),
        zero_percent: if difference == 0.0 {
            0.0
        } else {
            -minimum / difference * 100.0
        },
    }
}

fn set_overview_mode(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    element(document, "pe-overview-chart")?
        .dyn_into::<HtmlElement>()?
        .set_hidden(model.state.pe_mode != "chart");
    element(document, "pe-overview-grid")?
        .dyn_into::<HtmlElement>()?
        .set_hidden(model.state.pe_mode != "grid");
    let buttons = document.query_selector_all("[data-pe-overview-mode]")?;
    for index in 0..buttons.length() {
        if let Some(button) = buttons
            .item(index)
            .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
        {
            let pressed = button.get_attribute("data-pe-overview-mode").as_deref()
                == Some(&model.state.pe_mode);
            button.set_attribute("aria-pressed", if pressed { "true" } else { "false" })?;
        }
    }
    Ok(())
}

fn set_axes(document: &Document, rows: usize, cols: usize) -> Result<(), JsValue> {
    let row_labels = (0..rows)
        .map(|value| format!("<span>{value}</span>"))
        .collect::<String>();
    let col_labels = (0..cols)
        .map(|value| format!("<span>{value}</span>"))
        .collect::<String>();
    set_html(document, "row-axis", &row_labels)?;
    set_html(document, "col-axis", &col_labels)
}

fn set_grid_dimensions(document: &Document, rows: usize, cols: usize) -> Result<(), JsValue> {
    let columns = format!("repeat({cols}, clamp(14px, 3.8vw, 34px))");
    style(document, "pe-grid")?.set_property("grid-template-columns", &columns)?;
    style(document, "col-axis")?.set_property("grid-template-columns", &columns)?;
    style(document, "row-axis")?.set_property(
        "grid-template-rows",
        &format!("repeat({rows}, clamp(14px, 3.8vw, 34px))"),
    )
}

fn platform_markup(pe: &PeSummary) -> String {
    pe.platform_config.as_ref().map_or_else(
        || "<p>Platform: no platform PE entry</p>".into(),
        |config| {
            format!(
                "<p>Platform: {}, active requests {}, LSU {} bytes</p>",
                escape(&config.memory_map),
                config
                    .num_active_requests
                    .map_or_else(|| "n/a".into(), |value| integer(value as u64)),
                config
                    .lsu_access_bytes
                    .map_or_else(|| "n/a".into(), |value| integer(value as u64)),
            )
        },
    )
}

fn overlay_markup(model: &AppModel, pe: &PeSummary) -> String {
    pe.overlays
        .iter()
        .map(|(name, value)| {
            let metadata = model.data.overlay_metrics.get(name);
            let text = metadata
                .and_then(|value| value.label.as_deref())
                .unwrap_or(name);
            let unit = metadata
                .and_then(|value| value.unit.as_deref())
                .unwrap_or("");
            format!(
                "<span class=\"pill\">{}: {} {}</span>",
                escape(text),
                decimal(*value, 2),
                escape(unit)
            )
        })
        .collect()
}

fn style(document: &Document, id: &str) -> Result<web_sys::CssStyleDeclaration, JsValue> {
    Ok(element(document, id)?.dyn_into::<HtmlElement>()?.style())
}
