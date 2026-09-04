// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Document, Element, HtmlElement, HtmlInputElement, HtmlSelectElement};

use super::format::{bytes_u64, count, count_u64, decimal, escape, integer};
use super::logic::{AppModel, LayerAggregate};
use super::state::{EntityKind, RelationshipMode};
use crate::model::{LayerSummary, MachineOpSummary};

mod memory;
mod pe;
mod relationships;
mod tensors;

pub(crate) const ALL_PANELS: &[&str] = &[
    "timetable-summary",
    "layer-summary",
    "layer-details",
    "compute-summary",
    "pe-grid",
    "selected-pe",
    "tensor-memory",
    "selected-tensor",
    "memory-summary",
    "memories-overview",
    "memory-details",
    "relationships",
];

pub(crate) fn initialize_controls(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    set_text(document, "source-path", &model.data.summary.timetable)?;
    set_html(document, "pe-overview-measure", &pe_measure_options(model))?;
    select(document, "pe-overview-measure")?.set_value(&model.state.pe_measure.name());
    update_relationship_measure_options(model, document)?;
    update_filter_summaries(model, document)?;
    Ok(())
}

pub(crate) fn render_dirty(
    model: &AppModel,
    document: &Document,
    panels: &[&'static str],
    global_stats_dirty: bool,
    warnings_dirty: bool,
) -> Result<Vec<&'static str>, JsValue> {
    if global_stats_dirty {
        render_global_stats(model, document)?;
    }
    let rendered = render_panels_inner(model, document, panels)?;
    if warnings_dirty {
        render_warnings(model, document)?;
    }
    mark_render_complete(document).map(|()| rendered)
}

fn render_panels_inner(
    model: &AppModel,
    document: &Document,
    panels: &[&'static str],
) -> Result<Vec<&'static str>, JsValue> {
    let mut rendered = Vec::new();
    for panel in panels {
        if render_panel(model, document, panel)? {
            rendered.push(*panel);
        }
    }
    Ok(rendered)
}

fn render_panel(model: &AppModel, document: &Document, panel: &str) -> Result<bool, JsValue> {
    if !panel_is_visible(document, panel)? {
        return Ok(false);
    }
    match panel {
        "timetable-summary" => render_timetable_summary(model, document),
        "layer-summary" => render_layer_summary(model, document),
        "layer-details" => render_layer_detail(model, document),
        "compute-summary" => render_compute_summary(model, document),
        "pe-grid" => render_pe_overview(model, document),
        "selected-pe" => render_selected_pe(model, document),
        "tensor-memory" => render_tensor_memory(model, document),
        "selected-tensor" => render_selected_tensor(model, document),
        "memory-summary" => render_memory_summary(model, document),
        "memories-overview" => render_memories_overview(model, document),
        "memory-details" => render_memory_detail(model, document),
        "relationships" => render_relationships(model, document),
        _ => Ok(()),
    }?;
    Ok(true)
}

fn mark_render_complete(document: &Document) -> Result<(), JsValue> {
    let Some(root) = document.document_element() else {
        return Ok(());
    };
    let generation = root
        .get_attribute("data-gwr-render-generation")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
        .wrapping_add(1);
    root.set_attribute("data-gwr-render-generation", &generation.to_string())
}

pub(crate) fn update_filter_summaries(
    model: &AppModel,
    document: &Document,
) -> Result<(), JsValue> {
    for (kind, noun, id) in [
        (EntityKind::Layer, "layers", "layer-filter-summary"),
        (EntityKind::Pe, "PEs", "pe-filter-summary"),
        (EntityKind::Memory, "memories", "memory-filter-summary"),
        (EntityKind::Tensor, "tensors", "tensor-filter-summary"),
    ] {
        let filter = model.state.filter(kind);
        let text = if filter.is_all() {
            format!("All {}", integer(model.filter_value_count(kind)))
        } else if filter.selected_count() == 0 {
            "None".to_string()
        } else if filter.selected_count() == 1 {
            filter
                .values()
                .iter()
                .find(|value| filter.is_selected(value))
                .cloned()
                .unwrap_or_default()
        } else {
            format!("{} {noun}", integer(filter.selected_count() as u64))
        };
        set_text(document, id, &text)?;
    }
    Ok(())
}

pub(crate) fn render_filter_options(
    model: &AppModel,
    document: &Document,
    kind: EntityKind,
) -> Result<(), JsValue> {
    let filter = model.state.filter(kind);
    let (container_id, status_id) = filter_ids(kind);
    let matches = match filter.matches_pattern() {
        Ok(matches) => matches,
        Err(_) => {
            input(document, &format!("{}-filter-pattern", kind.name()))?
                .set_attribute("aria-invalid", "true")?;
            set_text(document, status_id, "Invalid regular expression")?;
            return Ok(());
        }
    };
    input(document, &format!("{}-filter-pattern", kind.name()))?
        .remove_attribute("aria-invalid")?;
    const WINDOW_THRESHOLD: usize = 1_000;
    const WINDOW_SIZE: usize = 500;
    let displayed = if matches.len() > WINDOW_THRESHOLD {
        &matches[..WINDOW_SIZE]
    } else {
        &matches
    };
    let mut html = String::new();
    for value in displayed {
        let checked = if filter.is_selected(value) {
            " checked"
        } else {
            ""
        };
        write!(
            html,
            "<label class=\"filter-option\"><input type=\"checkbox\" data-filter-kind=\"{}\" value=\"{}\"{}><span>{}</span></label>",
            kind.name(),
            escape(value),
            checked,
            escape(value),
        )
        .unwrap();
    }
    set_html(document, container_id, &html)?;
    let status = if displayed.len() == matches.len() {
        format!("{} shown", integer(matches.len() as u64))
    } else {
        format!(
            "First {} of {} matches shown; refine the expression to see others",
            integer(displayed.len() as u64),
            integer(matches.len() as u64)
        )
    };
    set_text(document, status_id, &status)
}

pub(crate) fn update_relationship_measure_options(
    model: &AppModel,
    document: &Document,
) -> Result<(), JsValue> {
    let values: Vec<(&str, String)> = match model.state.relationship_mode {
        RelationshipMode::Compute => std::iter::once(("machine-ops", "Machine ops".into()))
            .chain(std::iter::once(("nodes", "Compute nodes".into())))
            .chain(
                model
                    .data
                    .machine_ops
                    .iter()
                    .map(|operation| (operation.name.as_str(), operation.label.clone())),
            )
            .collect(),
        _ => vec![("read", "Read".into()), ("write", "Written".into())],
    };
    let html = values
        .iter()
        .map(|(value, text)| {
            format!(
                "<option value=\"{}\">{}</option>",
                escape(value),
                escape(text)
            )
        })
        .collect::<String>();
    set_html(document, "relationship-measure", &html)?;
    let select = select(document, "relationship-measure")?;
    if values
        .iter()
        .any(|(value, _)| *value == model.state.relationship_measure.name())
    {
        select.set_value(model.state.relationship_measure.name());
    } else if let Some((value, _)) = values.first() {
        select.set_value(value);
    }
    Ok(())
}

fn render_global_stats(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let summary = model.filtered_summary();
    set_text(
        document,
        "stat-machine-ops",
        &count_u64(summary.machine_ops.total),
    )?;
    set_text(
        document,
        "stat-compute",
        &format!("{} compute nodes", integer(summary.compute_nodes)),
    )?;
    set_text(document, "stat-tensors", &integer(summary.tensors))?;
    set_text(document, "stat-read-bytes", &bytes_u64(summary.read_bytes))?;
    set_text(
        document,
        "stat-write-bytes",
        &bytes_u64(summary.write_bytes),
    )?;
    set_text(document, "stat-edges", &integer(summary.edges))?;
    set_text(document, "stat-pes", &integer(summary.active_pes))
}

fn render_timetable_summary(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let summary = model.filtered_summary();
    let html = format!(
        "{}{}{}",
        metric_breakdown(
            "Layers",
            integer(model.filtered_layers().count() as u64),
            &[
                ("Read", bytes_u64(summary.read_bytes)),
                ("Written", bytes_u64(summary.write_bytes))
            ],
        ),
        compute_nodes_markup(model, summary.compute_nodes, &summary.by_op),
        machine_ops_markup(model, &summary.machine_ops),
    );
    set_html(document, "timetable-summary", &html)
}

fn render_layer_summary(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let layers = model.filtered_layers().collect::<Vec<_>>();
    if layers.is_empty() {
        return set_html(document, "layer-summary", "<p>No graph layers found.</p>");
    }
    let metrics = layers
        .iter()
        .map(|layer| (*layer, layer_metric(model, layer)))
        .collect::<Vec<_>>();
    let maxima = comparison_maxima(metrics.iter().map(|(_, aggregate)| aggregate));
    let mut rows = String::from("<div class=\"layer-summary-list\">");
    for index in visible_layer_indices(model, &metrics) {
        let (layer, aggregate) = &metrics[index];
        let selected = model.state.selected_layer.as_deref() == Some(layer.name.as_str());
        write!(
            rows,
            "<button type=\"button\" class=\"layer-summary-row comparison-row{}\" data-select-kind=\"layer\" data-select-id=\"{}\" data-selection-kind=\"layer\" data-selection-id=\"{}\" aria-pressed=\"{}\" aria-label=\"{}: {} compute nodes, {} machine ops, {} read, {} written\"><div class=\"comparison-heading\"><strong>{}</strong><span>{} PEs · {} tensors</span></div>{}</button>",
            selected.then_some(" selected").unwrap_or(""),
            escape(&layer.name),
            escape(&layer.name),
            selected,
            escape(&layer.name),
            integer(aggregate.compute_nodes),
            integer(aggregate.machine_ops.total),
            bytes_u64(aggregate.read_bytes),
            bytes_u64(aggregate.write_bytes),
            escape(&layer.name),
            integer(aggregate.active_pes.len() as u64),
            integer(aggregate.tensor_count),
            comparison_bars(&aggregate, maxima),
        )
        .unwrap();
    }
    rows.push_str("</div>");
    if metrics.len() > LAYER_WINDOW_SIZE {
        write!(
            rows,
            "<p class=\"filter-status\">Showing {} of {} layers; narrow the Layer filter to display others.</p>",
            integer(LAYER_WINDOW_SIZE as u64),
            integer(metrics.len() as u64),
        )
        .unwrap();
    }
    set_html(document, "layer-summary", &rows)
}

const LAYER_WINDOW_SIZE: usize = 500;

fn visible_layer_indices(
    model: &AppModel,
    metrics: &[(&LayerSummary, LayerAggregate)],
) -> Vec<usize> {
    let shown = metrics.len().min(LAYER_WINDOW_SIZE);
    let mut indices = (0..shown).collect::<Vec<_>>();
    let selected = model.state.selected_layer.as_deref();
    let selected_index = metrics
        .iter()
        .position(|(layer, _)| Some(layer.name.as_str()) == selected);
    if let (Some(index), Some(last)) = (
        selected_index.filter(|index| *index >= shown),
        indices.last_mut(),
    ) {
        *last = index;
    }
    indices
}

fn layer_metric(model: &AppModel, layer: &LayerSummary) -> LayerAggregate {
    let mut aggregate = model.layer_aggregate(layer);
    let context = model.context(Some(&layer.name), None);
    aggregate.tensor_count =
        u64::try_from(context.tensor_indices.len()).expect("Wasm collection lengths fit in u64");
    aggregate.read_bytes = context.read_bytes;
    aggregate.write_bytes = context.write_bytes;
    aggregate
}

fn render_layer_detail(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let Some(layer) = selected_visible_layer(model) else {
        return set_html(document, "layer-detail", "<p>No graph layer selected.</p>");
    };
    let aggregate = layer_metric(model, layer);
    let mut pe_rows = layer
        .pes
        .iter()
        .filter(|pe| model.state.pes.is_selected(&pe.name))
        .collect::<Vec<_>>();
    pe_rows.sort_by_key(|pe| {
        let global = model.pe(&pe.name);
        (
            global.map_or(0, |pe| pe.row),
            global.map_or(0, |pe| pe.col),
            pe.name.as_str(),
        )
    });
    let pe_metrics = pe_rows
        .iter()
        .map(|pe| layer_pe_metric(model, layer, pe))
        .collect::<Vec<_>>();
    let pe_maxima = comparison_maxima(pe_metrics.iter());
    let mut html = format!(
        "<div class=\"layer-detail-heading\"><h3>{}</h3><span>{} PEs</span></div><div class=\"layer-detail-metrics\"><div><span>Read</span><strong>{}</strong></div><div><span>Written</span><strong>{}</strong></div></div>{}{}<div class=\"layer-pe-summary-list\">",
        escape(&layer.name),
        integer(aggregate.active_pes.len() as u64),
        bytes_u64(aggregate.read_bytes),
        bytes_u64(aggregate.write_bytes),
        compute_nodes_markup(model, aggregate.compute_nodes, &aggregate.by_op),
        machine_ops_markup(model, &aggregate.machine_ops),
    );
    if pe_rows.is_empty() {
        html.push_str("<p>No processing elements in this layer.</p>");
    }
    for pe in pe_rows {
        let metric = layer_pe_metric(model, layer, pe);
        let selected = model.state.selected_pe.as_deref() == Some(pe.name.as_str());
        write!(
            html,
            "<button type=\"button\" class=\"layer-pe-summary-row comparison-row{}\" data-select-kind=\"pe\" data-select-id=\"{}\" data-selection-kind=\"pe\" data-selection-id=\"{}\" aria-pressed=\"{}\" aria-label=\"{}: {} compute nodes, {} machine ops, {} read, {} written\"><div class=\"comparison-heading\"><strong>{}</strong><span>{} tensors</span></div>{}</button>",
            selected.then_some(" selected").unwrap_or(""),
            escape(&pe.name),
            escape(&pe.name),
            selected,
            escape(&pe.name),
            integer(metric.compute_nodes),
            integer(metric.machine_ops.total),
            bytes_u64(metric.read_bytes),
            bytes_u64(metric.write_bytes),
            escape(&pe.name),
            integer(metric.tensor_count),
            comparison_bars(&metric, pe_maxima),
        )
        .unwrap();
    }
    html.push_str("</div>");
    set_html(document, "layer-detail", &html)
}

fn layer_pe_metric(
    model: &AppModel,
    layer: &LayerSummary,
    pe: &crate::model::LayerPeSummary,
) -> LayerAggregate {
    let context = model.context(Some(&layer.name), Some(&pe.name));
    LayerAggregate {
        compute_nodes: pe.compute_nodes,
        machine_ops: pe.machine_ops.clone(),
        tensor_count: u64::try_from(context.tensor_indices.len())
            .expect("Wasm collection lengths fit in u64"),
        read_bytes: context.read_bytes,
        write_bytes: context.write_bytes,
        ..LayerAggregate::default()
    }
}

fn render_compute_summary(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let population = model.compute_population();
    let operations = population
        .iter()
        .map(|pe| model.machine_ops_for_pe(pe))
        .collect::<Vec<_>>();
    let values = operations
        .iter()
        .map(|ops| ops.total as f64)
        .collect::<Vec<_>>();
    let total = values.iter().sum::<f64>();
    let maximum = values.iter().copied().fold(0.0_f64, f64::max);
    let average = if values.is_empty() {
        0.0
    } else {
        total / values.len() as f64
    };
    let allocated = values.iter().filter(|value| **value > 0.0).count();
    let imbalance = if average == 0.0 {
        0.0
    } else {
        maximum / average
    };
    let combined = operations
        .iter()
        .fold(MachineOpSummary::default(), |mut total, value| {
            total.total += value.total;
            total.adds += value.adds;
            total.muls += value.muls;
            total.compares += value.compares;
            total
        });
    let layer_count = model.filtered_layers().count();
    let layer_label = if layer_count == model.data.layers.len() {
        "All layers".into()
    } else {
        format!("{} layers", integer(layer_count as u64))
    };
    let html = format!(
        "<div class=\"compute-summary-context\"><strong>Machine ops</strong><span>{}</span></div><div class=\"compute-summary-metrics\"><div><span>Total</span><strong>{}</strong></div><div><span>Average per PE</span><strong>{}</strong></div><div><span>Maximum</span><strong>{}</strong></div><div><span>Max / average</span><strong>{}×</strong></div><div><span>Allocated PEs</span><strong>{} / {}</strong></div></div>{}",
        escape(layer_label),
        count(total),
        count(average),
        count(maximum),
        decimal(imbalance, 2),
        integer(allocated as u64),
        integer(population.len() as u64),
        machine_ops_markup(model, &combined),
    );
    set_html(document, "compute-summary", &html)
}

fn render_warnings(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let html = model
        .data
        .warnings
        .iter()
        .map(|warning| format!("<p>{}</p>", escape(warning)))
        .collect::<String>();
    set_html(document, "warnings", &html)
}

fn selected_visible_layer(model: &AppModel) -> Option<&LayerSummary> {
    model
        .selected_layer()
        .filter(|layer| model.state.layers.is_selected(&layer.name))
        .or_else(|| model.filtered_layers().next())
}

fn metric_breakdown(label_text: &str, total: String, entries: &[(&str, String)]) -> String {
    let aria_breakdown = entries
        .iter()
        .map(|(name, value)| format!("{value} {name}"))
        .collect::<Vec<_>>()
        .join(", ");
    let aria_label = format!(
        "{} {}{}",
        total,
        label_text,
        (!aria_breakdown.is_empty())
            .then(|| format!(": {aria_breakdown}"))
            .unwrap_or_default()
    );
    let breakdown = entries
        .iter()
        .map(|(name, value)| {
            format!(
                "<div><span>{}</span><strong>{}</strong></div>",
                escape(name),
                value
            )
        })
        .collect::<String>();
    format!(
        "<div class=\"metric-breakdown-summary\" aria-label=\"{}\"><div class=\"total\"><span>{}</span><strong>{}</strong></div>{breakdown}</div>",
        escape(aria_label),
        escape(label_text),
        total
    )
}

fn machine_ops_markup(model: &AppModel, operations: &MachineOpSummary) -> String {
    let entries = model
        .data
        .machine_ops
        .iter()
        .map(|operation| {
            let value = match operation.name.as_str() {
                "adds" => operations.adds,
                "muls" => operations.muls,
                "compares" => operations.compares,
                _ => 0,
            };
            (operation.label.as_str(), count_u64(value))
        })
        .collect::<Vec<_>>();
    metric_breakdown("Machine ops", count_u64(operations.total), &entries)
}

fn compute_nodes_markup(model: &AppModel, total: u64, by_op: &BTreeMap<String, u64>) -> String {
    let entries = model
        .data
        .ops
        .iter()
        .map(|operation| {
            (
                operation.as_str(),
                count_u64(*by_op.get(operation).unwrap_or(&0)),
            )
        })
        .collect::<Vec<_>>();
    metric_breakdown("Compute nodes", integer(total), &entries)
}

#[derive(Clone, Copy)]
struct Maxima {
    nodes: f64,
    operations: f64,
    read: f64,
    write: f64,
}

fn comparison_maxima<'a>(metrics: impl Iterator<Item = &'a LayerAggregate>) -> Maxima {
    metrics.fold(
        Maxima {
            nodes: 1.0,
            operations: 1.0,
            read: 1.0,
            write: 1.0,
        },
        |maxima, metric| Maxima {
            nodes: maxima.nodes.max(metric.compute_nodes as f64),
            operations: maxima.operations.max(metric.machine_ops.total as f64),
            read: maxima.read.max(metric.read_bytes as f64),
            write: maxima.write.max(metric.write_bytes as f64),
        },
    )
}

fn comparison_bars(metric: &LayerAggregate, maxima: Maxima) -> String {
    comparison_metrics(&[
        (
            "Compute nodes",
            metric.compute_nodes as f64,
            integer(metric.compute_nodes),
            "nodes",
            maxima.nodes,
        ),
        (
            "Machine ops",
            metric.machine_ops.total as f64,
            count_u64(metric.machine_ops.total),
            "ops",
            maxima.operations,
        ),
        (
            "Read",
            metric.read_bytes as f64,
            bytes_u64(metric.read_bytes),
            "read",
            maxima.read,
        ),
        (
            "Written",
            metric.write_bytes as f64,
            bytes_u64(metric.write_bytes),
            "write",
            maxima.write,
        ),
    ])
}

fn comparison_metrics(items: &[(&str, f64, String, &str, f64)]) -> String {
    let rows = items.iter().map(|(label_text, value, formatted, mode, maximum)| format!(
        "<div class=\"comparison-metric\"><div><span>{}</span><strong>{}</strong></div><div class=\"comparison-track {}\"><div style=\"width: {}%\"></div></div></div>",
        escape(label_text), formatted, mode, value / maximum.max(1.0) * 100.0,
    )).collect::<String>();
    format!("<div class=\"comparison-metrics\">{rows}</div>")
}

fn panel_is_visible(document: &Document, view: &str) -> Result<bool, JsValue> {
    let selector = format!("[data-view=\"{view}\"]");
    Ok(document
        .query_selector(&selector)?
        .and_then(|element| element.dyn_into::<HtmlElement>().ok())
        .is_some_and(|element| !element.hidden()))
}

fn set_html(document: &Document, id: &str, html: &str) -> Result<(), JsValue> {
    element(document, id)?.set_inner_html(html);
    Ok(())
}

fn set_text(document: &Document, id: &str, text: &str) -> Result<(), JsValue> {
    element(document, id)?.set_text_content(Some(text));
    Ok(())
}

fn element(document: &Document, id: &str) -> Result<Element, JsValue> {
    document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("Missing report element #{id}")))
}

fn input(document: &Document, id: &str) -> Result<HtmlInputElement, JsValue> {
    element(document, id)?
        .dyn_into()
        .map_err(|_| JsValue::from_str(&format!("#{id} is not an input")))
}

fn select(document: &Document, id: &str) -> Result<HtmlSelectElement, JsValue> {
    element(document, id)?
        .dyn_into()
        .map_err(|_| JsValue::from_str(&format!("#{id} is not a select")))
}

fn filter_ids(kind: EntityKind) -> (&'static str, &'static str) {
    match kind {
        EntityKind::Layer => ("layer-filter", "layer-filter-pattern-status"),
        EntityKind::Pe => ("pe-filter", "pe-filter-pattern-status"),
        EntityKind::Memory => ("memory-filter", "memory-filter-pattern-status"),
        EntityKind::Tensor => ("tensor-filter", "tensor-filter-pattern-status"),
    }
}

fn pe_measure_options(model: &AppModel) -> String {
    let compute_ops = model
        .data
        .machine_ops
        .iter()
        .map(|operation| {
            format!(
                "<option value=\"compute:machine-op:{}\">Compute allocation · {}</option>",
                escape(&operation.name),
                escape(&operation.label)
            )
        })
        .collect::<String>();
    let metrics = model
        .data
        .overlay_metrics
        .iter()
        .map(|(name, metadata)| {
            format!(
                "<option value=\"metric:{}\">Metrics file · {}</option>",
                escape(name),
                escape(metadata.label.as_deref().unwrap_or(name))
            )
        })
        .collect::<String>();
    format!(
        "<optgroup label=\"Compute allocation\"><option value=\"compute:machine-ops\">Compute allocation · Machine ops</option><option value=\"compute:compute-nodes\">Compute allocation · Compute nodes</option>{compute_ops}</optgroup><optgroup label=\"Data\"><option value=\"data:total\">Data · Total</option><option value=\"data:read\">Data · Read</option><option value=\"data:write\">Data · Written</option></optgroup><optgroup label=\"Selected tensor\"><option value=\"tensor:read\">Selected tensor · Read bytes</option><option value=\"tensor:write\">Selected tensor · Written bytes</option></optgroup><optgroup label=\"Metrics file\">{metrics}</optgroup>"
    )
}

fn render_pe_overview(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    pe::render_overview(model, document)
}

fn render_selected_pe(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    pe::render_detail(model, document)
}

fn render_tensor_memory(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    tensors::render_memory_map(model, document)
}

fn render_selected_tensor(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    tensors::render_detail(model, document)
}

fn render_memory_summary(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    memory::render_summary(model, document)
}

fn render_memories_overview(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    memory::render_overview(model, document)
}

fn render_memory_detail(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    memory::render_detail(model, document)
}

fn render_relationships(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    relationships::render(model, document)
}
