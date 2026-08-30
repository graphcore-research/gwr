// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashSet;
use std::fmt::Write as _;

use wasm_bindgen::JsValue;
use web_sys::Document;

use super::super::address::{
    AddressRange, MemoryRegion, TensorLayout, build_regions, clipped_range, range_union_length,
};
use super::super::format::{bytes, bytes_u64, bytes_u128, decimal, escape, hex, hex_u128, integer};
use super::super::logic::AppModel;
use super::set_html;
use super::tensors::{render_row_limit, tensor_row, tensor_row_window};
use crate::model::MemoryDeviceSummary;

pub(super) fn render_summary(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let metrics = memory_metrics(model);
    if metrics.is_empty() {
        return set_html(document, "memory-summary", &empty_message(model));
    }
    let totals = totals(&metrics);
    let allocated_percent = totals.allocated as f64 / totals.capacity.max(1) as f64 * 100.0;
    let capacity = bytes_u128(totals.capacity);
    let allocated = bytes_u128(totals.allocated);
    let read = bytes_u128(totals.read);
    let written = bytes_u128(totals.write);
    let allocated_percent = format!("{allocated_percent:.3}");
    let aria_label = format!(
        "{} Memories: {capacity} Capacity, {allocated} ({allocated_percent}%) Allocated, {read} Read, {written} Written",
        integer(metrics.len() as u64),
    );
    let html = format!(
        "<div class=\"metric-breakdown-summary\" aria-label=\"{}\"><div class=\"total\"><span>Memories</span><strong>{}</strong></div><div><span>Capacity</span><strong>{}</strong></div><div><span>Allocated</span><strong>{} ({}%)</strong></div><div><span>Read</span><strong>{}</strong></div><div><span>Written</span><strong>{}</strong></div></div>",
        escape(aria_label),
        integer(metrics.len() as u64),
        capacity,
        allocated,
        allocated_percent,
        read,
        written,
    );
    set_html(document, "memory-summary", &html)
}

pub(super) fn render_overview(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let metrics = memory_metrics(model);
    if metrics.is_empty() {
        return set_html(document, "memories-overview", &empty_message(model));
    }
    let totals = totals(&metrics);
    let maximum_allocated = metrics
        .iter()
        .map(|value| value.allocated)
        .fold(1_u128, u128::max);
    let maximum_read = metrics
        .iter()
        .map(|value| u128::from(value.read))
        .fold(1_u128, u128::max);
    let maximum_write = metrics
        .iter()
        .map(|value| u128::from(value.write))
        .fold(1_u128, u128::max);
    let average_read = totals.read as f64 / metrics.len() as f64;
    let average_write = totals.write as f64 / metrics.len() as f64;
    let selected = selected_memory(model, &metrics).map(|value| value.memory.name.clone());
    let mut html = String::from("<div class=\"memories-overview-list\">");
    for value in metrics {
        let is_selected = selected.as_deref() == Some(value.memory.name.as_str());
        let capacity = value.memory.capacity_bytes.max(1);
        write!(
            html,
            "<button type=\"button\" class=\"memories-overview-row comparison-row{}\" data-select-kind=\"memory\" data-select-id=\"{}\" data-selection-kind=\"memory\" data-selection-id=\"{}\" aria-pressed=\"{}\" aria-label=\"{}: {} allocated, {} read, {} written\"><div class=\"comparison-heading\"><strong>{}</strong><span>{} · {} tensors</span></div><div class=\"comparison-metrics memory-comparison-metrics\">{}{}{}</div></button>",
            is_selected.then_some(" selected").unwrap_or(""),
            escape(&value.memory.name),
            escape(&value.memory.name),
            is_selected,
            escape(&value.memory.name),
            bytes_u128(value.allocated),
            bytes_u64(value.read),
            bytes_u64(value.write),
            escape(&value.memory.name),
            escape(&value.memory.kind),
            integer(value.tensor_indices.len() as u64),
            comparison("Allocated", value.allocated as f64, format!("{} <em>{}%</em>", bytes_u128(value.allocated), decimal(value.allocated as f64 / capacity as f64 * 100.0, 3)), "allocated", maximum_allocated as f64, None),
            comparison("Read", value.read as f64, bytes_u64(value.read), "read", maximum_read as f64, Some(average_read / maximum_read as f64 * 100.0)),
            comparison("Written", value.write as f64, bytes_u64(value.write), "write", maximum_write as f64, Some(average_write / maximum_write as f64 * 100.0)),
        )
        .unwrap();
    }
    html.push_str("</div>");
    set_html(document, "memories-overview", &html)
}

pub(super) fn render_detail(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let metrics = memory_metrics(model);
    let Some(selected) = selected_memory(model, &metrics) else {
        return set_html(document, "memory-detail", &empty_message(model));
    };
    let totals = totals(&metrics);
    let average_read = totals.read as f64 / metrics.len() as f64;
    let average_write = totals.write as f64 / metrics.len() as f64;
    let maximum_read = metrics.iter().map(|value| value.read).fold(1_u64, u64::max);
    let maximum_write = metrics
        .iter()
        .map(|value| value.write)
        .fold(1_u64, u64::max);
    let capacity = selected.memory.capacity_bytes.max(1);
    let allocated_percent = (selected.allocated as f64 / capacity as f64 * 100.0).min(100.0);
    let memory_range = AddressRange::new(selected.memory.base_addr, selected.memory.capacity_bytes);
    let mut html = format!(
        "<div class=\"memory-detail-list\"><section class=\"memory-detail-card\"><div class=\"memory-detail-header\"><div><h3>{}</h3><span>{} · {} - {}</span></div><strong>{} / {} allocated ({}%)</strong></div><div class=\"memory-detail-meter\"><div style=\"width: {allocated_percent}%\"></div></div><div class=\"memory-detail-traffic\">{}{} </div><div class=\"memory-detail-layout\">",
        escape(&selected.memory.name),
        escape(&selected.memory.kind),
        hex(selected.memory.base_addr),
        hex_u128(memory_range.end),
        bytes_u128(selected.allocated),
        bytes_u64(capacity),
        decimal(allocated_percent, 3),
        traffic_row("Read", selected.read, average_read, maximum_read, "read"),
        traffic_row(
            "Written",
            selected.write,
            average_write,
            maximum_write,
            "write"
        ),
    );
    let regions = memory_regions(model, selected);
    if regions.is_empty() {
        html.push_str("<p class=\"memory-empty\">No tensors allocated in this memory.</p>");
    } else {
        let selected_tensor = model
            .state
            .selected_tensor
            .as_deref()
            .and_then(|id| model.tensor_index(id));
        let window = tensor_row_window(
            regions.iter().flat_map(|region| region.tensors.iter()),
            selected_tensor,
        );
        for region in &regions {
            if !region
                .tensors
                .iter()
                .any(|layout| window.contains(layout.tensor_index))
            {
                continue;
            }
            if region.gap_before > 0 {
                write!(
                    html,
                    "<div class=\"memory-gap-row\">{} unused</div>",
                    bytes_u128(region.gap_before)
                )
                .unwrap();
            }
            write!(
                html,
                "<div class=\"memory-region-header\"><span>{} - {}</span><strong>{} allocated</strong></div>",
                hex_u128(region.start),
                hex_u128(region.end),
                bytes_u128(region.allocated),
            )
            .unwrap();
            for layout in &region.tensors {
                if window.contains(layout.tensor_index) {
                    html.push_str(&tensor_row(
                        model,
                        layout,
                        region,
                        Some(&selected.memory.name),
                    ));
                }
            }
        }
        render_row_limit(&mut html, &window);
    }
    html.push_str("</div></section></div>");
    set_html(document, "memory-detail", &html)
}

struct MemoryMetrics<'a> {
    memory: &'a MemoryDeviceSummary,
    tensor_indices: Vec<usize>,
    allocated: u128,
    read: u64,
    write: u64,
}

#[derive(Default)]
struct Totals {
    capacity: u128,
    allocated: u128,
    read: u128,
    write: u128,
}

fn memory_metrics(model: &AppModel) -> Vec<MemoryMetrics<'_>> {
    if model.state.layers.is_all() && model.state.pes.is_all() && model.state.tensors.is_all() {
        return model
            .data
            .memory
            .platform_memories
            .iter()
            .filter(|memory| model.state.memories.is_selected(&memory.name))
            .map(|memory| MemoryMetrics {
                memory,
                tensor_indices: memory
                    .tensors
                    .iter()
                    .filter_map(|id| model.tensor_index(id))
                    .collect(),
                allocated: u128::from(memory.allocated_bytes),
                read: memory.read_bytes,
                write: memory.write_bytes,
            })
            .collect();
    }
    let visible = model.filtered_tensors();
    let visible_ids = visible
        .iter()
        .map(|tensor| tensor.id.as_str())
        .collect::<HashSet<_>>();
    model
        .data
        .memory
        .platform_memories
        .iter()
        .filter(|memory| model.state.memories.is_selected(&memory.name))
        .map(|memory| {
            let mut allocation_ranges = Vec::new();
            let mut result = MemoryMetrics {
                memory,
                tensor_indices: Vec::new(),
                allocated: 0,
                read: 0,
                write: 0,
            };
            for (index, tensor) in model.data.tensors.iter().enumerate() {
                if !visible_ids.contains(tensor.id.as_str())
                    || !memory.tensors.iter().any(|id| id == &tensor.id)
                {
                    continue;
                }
                let Some((start, overlap)) = clipped_range(
                    tensor.addr,
                    tensor.num_bytes.max(1),
                    memory.base_addr,
                    memory.capacity_bytes,
                ) else {
                    continue;
                };
                allocation_ranges.push(AddressRange::new(start, overlap));
                let traffic = model.tensor_traffic_for(tensor, None, None, Some(&memory.name));
                result.tensor_indices.push(index);
                result.read += traffic.read_bytes;
                result.write += traffic.write_bytes;
            }
            result.allocated = range_union_length(allocation_ranges);
            result
        })
        .collect()
}

fn selected_memory<'a>(
    model: &AppModel,
    metrics: &'a [MemoryMetrics<'a>],
) -> Option<&'a MemoryMetrics<'a>> {
    let selected = model.state.selected_memory.as_deref();
    metrics
        .iter()
        .find(|value| Some(value.memory.name.as_str()) == selected)
        .or_else(|| metrics.first())
}

fn memory_regions(model: &AppModel, metrics: &MemoryMetrics<'_>) -> Vec<MemoryRegion> {
    let layouts = metrics
        .tensor_indices
        .iter()
        .filter_map(|index| {
            let tensor = &model.data.tensors[*index];
            clipped_range(
                tensor.addr,
                tensor.num_bytes.max(1),
                metrics.memory.base_addr,
                metrics.memory.capacity_bytes,
            )
            .map(|(address, bytes)| TensorLayout {
                tensor_index: *index,
                address,
                bytes,
            })
        })
        .collect();
    build_regions(layouts, &model.data.tensors, model.state.skip_memory_gaps)
}

fn totals(metrics: &[MemoryMetrics<'_>]) -> Totals {
    metrics.iter().fold(Totals::default(), |mut totals, value| {
        totals.capacity += u128::from(value.memory.capacity_bytes);
        totals.allocated += value.allocated;
        totals.read += u128::from(value.read);
        totals.write += u128::from(value.write);
        totals
    })
}

fn comparison(
    name: &str,
    value: f64,
    formatted: String,
    mode: &str,
    maximum: f64,
    marker: Option<f64>,
) -> String {
    let marker = marker
        .map(|position| {
            format!(
                "<i style=\"left: {}%\" aria-hidden=\"true\"></i>",
                position.clamp(0.0, 100.0)
            )
        })
        .unwrap_or_default();
    format!(
        "<div class=\"comparison-metric\"><div><span>{}</span><strong>{formatted}</strong></div><div class=\"comparison-track {}\"><div style=\"width: {}%\"></div>{marker}</div></div>",
        escape(name),
        escape(mode),
        value / maximum.max(1.0) * 100.0,
    )
}

fn traffic_row(name: &str, value: u64, average: f64, maximum: u64, mode: &str) -> String {
    format!(
        "<div class=\"memory-detail-traffic-row\"><span>{}</span><div class=\"memory-detail-traffic-track {}\"><div style=\"width: {}%\"></div><i style=\"left: {}%\" aria-hidden=\"true\"></i></div><strong>{}</strong><em>{}% of maximum · average {}</em></div>",
        escape(name),
        escape(mode),
        value as f64 / maximum as f64 * 100.0,
        average / maximum as f64 * 100.0,
        bytes_u64(value),
        decimal(value as f64 / maximum as f64 * 100.0, 1),
        bytes(average),
    )
}

fn empty_message(model: &AppModel) -> String {
    let text = if model.data.memory.platform_memories.is_empty() {
        "Provide a platform for memory details."
    } else {
        "No memories match the current filters."
    };
    format!("<p class=\"memory-empty\">{text}</p>")
}
