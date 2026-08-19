// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fmt::Write as _;

use wasm_bindgen::JsValue;
use web_sys::Document;

use super::super::format::{bytes_u64, bytes_u128, decimal, escape, hex, hex_u128, integer};
use super::super::logic::{AppModel, MemoryRegion, TensorLayout};
use super::set_html;
use crate::model::TensorSummary;

pub(super) fn render_memory_map(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let regions = model.memory_regions();
    if regions.is_empty() {
        return set_html(document, "tensor-memory", "No tensor nodes found.");
    }
    let mut html = String::from("<div class=\"memory-regions\">");
    for region in &regions {
        render_region(&mut html, model, region);
    }
    html.push_str("</div>");
    let allocated = regions.iter().fold(0_u128, |total, region| {
        total.saturating_add(region.allocated)
    });
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
    let title = tensor_title(model, tensor);
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

pub(super) fn tensor_title(model: &AppModel, tensor: &TensorSummary) -> String {
    let traffic = model.tensor_traffic(tensor);
    format!(
        "{}: {}, {}, read {}x, written {}x, consumed by {} PEs",
        tensor.id,
        hex(tensor.addr),
        bytes_u64(tensor.num_bytes),
        decimal(traffic.read_ratio, 2),
        decimal(traffic.write_ratio, 2),
        integer(tensor.consumption_by_pe.len() as u64),
    )
}

fn render_region(html: &mut String, model: &AppModel, region: &MemoryRegion) {
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
        html.push_str(&tensor_row(model, tensor, region, None));
    }
    html.push_str("</div></section>");
}
