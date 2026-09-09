// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use wasm_bindgen::JsValue;
use web_sys::Document;

use super::super::address::{MemoryRegion, TensorLayout};
use super::super::format::{bytes_u64, bytes_u128, decimal, escape, hex, hex_u128, integer};
use super::super::logic::{AppModel, TensorTraffic};
use super::set_html;
use crate::model::TensorSummary;

const MAX_TENSOR_ROWS: usize = 500;

pub(super) fn render_memory_map(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let regions = model.memory_regions();
    if regions.is_empty() {
        return set_html(document, "tensor-memory", "No tensor nodes found.");
    }
    let selected = model
        .state
        .selected_tensor
        .as_deref()
        .and_then(|id| model.tensor_index(id));
    let window = tensor_row_window(
        regions.iter().flat_map(|region| region.tensors.iter()),
        selected,
    );
    let mut html = String::from("<div class=\"memory-regions\">");
    for region in &regions {
        render_region(&mut html, model, region, &window);
    }
    html.push_str("</div>");
    render_row_limit(&mut html, &window);
    let allocated = regions.iter().map(|region| region.allocated).sum::<u128>();
    write!(
        html,
        "<div class=\"memory-legend\"><span><i class=\"tensor\"></i>tensor traffic</span><span><i class=\"read\"></i>read %</span><span><i class=\"write\"></i>written %</span><span><i class=\"gap\"></i>unused gap{}</span><strong>{} allocated</strong></div>",
        if model.state.skip_memory_gaps { " (collapsed between regions)" } else { "" },
        bytes_u128(allocated),
    )
    .unwrap();
    set_html(document, "tensor-memory", &html)
}

pub(super) fn render_detail(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let Some(tensor) = model.selected_tensor() else {
        return set_html(document, "selected-tensor", "No tensor selected.");
    };
    let traffic = model.tensor_traffic(tensor);
    let read = traffic.read_bytes;
    let written = traffic.write_bytes;
    let tensor_bytes = tensor.num_bytes;
    let maximum = tensor_bytes.max(read).max(written).max(1);
    let mut bars = String::new();
    for (name, value, ratio, mode) in [
        ("Tensor size", tensor_bytes, 1.0, "size"),
        (
            "Read",
            read,
            read as f64 / tensor_bytes.max(1) as f64,
            "read",
        ),
        (
            "Written",
            written,
            written as f64 / tensor_bytes.max(1) as f64,
            "write",
        ),
    ] {
        write!(
            bars,
            "<div class=\"tensor-byte-row\"><span>{name}</span><div class=\"tensor-byte-track\"><div class=\"tensor-byte-fill {mode}\" style=\"width: {}%\"></div></div><strong>{}</strong><em>{}×</em></div>",
            value as f64 / maximum as f64 * 100.0,
            bytes_u64(value),
            decimal(ratio, 2),
        )
        .unwrap();
    }
    let shape = tensor
        .shape
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" × ");
    let html = format!(
        "<h2>{}</h2><p>{} · {} · {} [{}]</p><div class=\"tensor-byte-summary\">{bars}</div>",
        escape(&tensor.id),
        hex(tensor.addr),
        bytes_u64(tensor_bytes),
        escape(&tensor.dtype),
        escape(shape),
    );
    set_html(document, "selected-tensor", &html)
}

pub(super) fn tensor_row(
    model: &AppModel,
    layout: &TensorLayout,
    region: &MemoryRegion,
    exact_memory: Option<&str>,
) -> String {
    let tensor = &model.data.tensors[layout.tensor_index];
    let traffic = model.tensor_traffic_for(tensor, None, None, exact_memory);
    let left = u128::from(layout.address).saturating_sub(region.start) as f64
        / region.span() as f64
        * 100.0;
    let width = (u128::from(layout.bytes) as f64 / region.span() as f64 * 100.0)
        .max(0.35)
        .min(100.0 - left);
    let selected = model.state.selected_tensor.as_deref() == Some(tensor.id.as_str());
    let title = tensor_title(tensor, &traffic);
    format!(
        "<div class=\"memory-tensor-row{}\" data-selection-kind=\"tensor\" data-selection-id=\"{}\"><button type=\"button\" class=\"memory-tensor-label\" title=\"{}\" data-select-kind=\"tensor\" data-select-id=\"{}\">{}</button><div class=\"memory-tensor-track\"><button type=\"button\" class=\"memory-tensor-block{}\" style=\"left: {left}%; width: {width}%; --write-share: {}%; --read-share: {}%\" title=\"{}\" aria-label=\"{}\" data-select-kind=\"tensor\" data-select-id=\"{}\"><span class=\"memory-tensor-fill read\"></span><span class=\"memory-tensor-fill write\"></span></button></div><span class=\"memory-tensor-size\" title=\"{}\">W {}% / R {}%</span></div>",
        selected.then_some(" selected").unwrap_or(""),
        escape(&tensor.id),
        escape(&tensor.id),
        escape(&tensor.id),
        escape(&tensor.id),
        selected.then_some(" selected").unwrap_or(""),
        (traffic.write_ratio * 100.0).min(100.0),
        (traffic.read_ratio * 100.0).min(100.0),
        escape(&title),
        escape(&title),
        escape(&tensor.id),
        bytes_u64(layout.bytes),
        traffic.write_ratio.mul_add(100.0, 0.0).round(),
        traffic.read_ratio.mul_add(100.0, 0.0).round(),
    )
}

pub(super) fn tensor_title(tensor: &TensorSummary, traffic: &TensorTraffic) -> String {
    format!(
        "{}: {}, {}, read {}x, written {}x, consumed by {} PEs",
        tensor.id,
        hex(tensor.addr),
        bytes_u64(tensor.num_bytes),
        decimal(traffic.read_ratio, 2),
        decimal(traffic.write_ratio, 2),
        integer(traffic.reads.len() as u64),
    )
}

pub(super) struct TensorRowWindow {
    tensors: BTreeSet<usize>,
    total: usize,
}

impl TensorRowWindow {
    pub(super) fn contains(&self, tensor: usize) -> bool {
        self.tensors.contains(&tensor)
    }
}

pub(super) fn tensor_row_window<'a>(
    layouts: impl Iterator<Item = &'a TensorLayout>,
    selected: Option<usize>,
) -> TensorRowWindow {
    let mut tensors = BTreeSet::new();
    let mut selected_present = false;
    let mut total = 0;
    for layout in layouts {
        total += 1;
        if tensors.len() < MAX_TENSOR_ROWS {
            tensors.insert(layout.tensor_index);
        }
        selected_present |= selected == Some(layout.tensor_index);
    }
    if selected_present {
        let selected = selected.expect("a selected tensor was found");
        if !tensors.contains(&selected) && tensors.len() == MAX_TENSOR_ROWS {
            tensors.pop_last();
        }
        tensors.insert(selected);
    }
    TensorRowWindow { tensors, total }
}

pub(super) fn render_row_limit(html: &mut String, window: &TensorRowWindow) {
    if window.total > window.tensors.len() {
        write!(
            html,
            "<p>Showing {} of {} tensors.</p>",
            integer(window.tensors.len() as u64),
            integer(window.total as u64),
        )
        .unwrap();
    }
}

fn render_region(
    html: &mut String,
    model: &AppModel,
    region: &MemoryRegion,
    window: &TensorRowWindow,
) {
    if !region
        .tensors
        .iter()
        .any(|layout| window.contains(layout.tensor_index))
    {
        return;
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
        "<section class=\"memory-region\"><div class=\"memory-region-header\"><strong>{}-{}</strong><span>{} tensors</span><span>{} allocated</span><span>{} span</span></div><div class=\"memory-region-tensors\">",
        hex_u128(region.start),
        hex_u128(region.end),
        integer(region.tensors.len() as u64),
        bytes_u128(region.allocated),
        bytes_u128(region.span()),
    )
    .unwrap();
    for tensor in &region.tensors {
        if window.contains(tensor.tensor_index) {
            html.push_str(&tensor_row(model, tensor, region, None));
        }
    }
    html.push_str("</div></section>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_tensor_rows_and_retains_the_selection() {
        let layouts = (0..600)
            .map(|tensor_index| TensorLayout {
                tensor_index,
                address: tensor_index as u64,
                bytes: 1,
            })
            .collect::<Vec<_>>();

        let window = tensor_row_window(layouts.iter(), Some(599));

        assert_eq!(window.tensors.len(), 500);
        assert!(window.contains(0));
        assert!(window.contains(599));
        assert!(!window.contains(598));
    }
}
