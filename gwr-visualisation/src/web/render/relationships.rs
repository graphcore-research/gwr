// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, HashMap};
use std::f64::consts::PI;
use std::fmt::Write as _;

use wasm_bindgen::{JsCast, JsValue};
use web_sys::{CanvasRenderingContext2d, Document, HtmlCanvasElement};

use super::super::format::{bytes, count, escape, integer};
use super::super::logic::AppModel;
use super::super::relationship_geometry::{
    Point, bezier_controls, edge_alpha, interpolate_hierarchy,
};
use super::set_html;
use crate::model::{MemoryDeviceSummary, TensorSummary};

const WIDTH: f64 = 1_000.0;
const HEIGHT: f64 = 620.0;
const CENTER_X: f64 = WIDTH / 2.0;
const CENTER_Y: f64 = HEIGHT / 2.0;
const LEAF_RADIUS: f64 = 250.0;
const GROUP_RADIUS: f64 = 132.0;
const MAX_LAYER_SOURCES: usize = 500;
const MAX_RENDERED_EDGES: usize = 5_000;

#[derive(Clone)]
struct Node {
    id: String,
    label: String,
    group: String,
    x: f64,
    y: f64,
    angle: f64,
}

struct Edge {
    source: String,
    target: String,
    value: f64,
}

struct RelationModel {
    sources: Vec<Node>,
    targets: Vec<Node>,
    edges: Vec<Edge>,
    source_label: &'static str,
    target_label: &'static str,
    omitted_sources: usize,
}

struct Group {
    point: Point,
    nodes: Vec<usize>,
}

pub(super) fn render(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let strength = model.state.relationship_strength;
    document
        .get_element_by_id("relationship-strength-value")
        .ok_or_else(|| JsValue::from_str("Missing relationship strength output"))?
        .set_text_content(Some(&format!("{strength}%")));
    if requires_platform(&model.state.relationship_mode)
        && model.data.memory.platform_memories.is_empty()
    {
        return set_html(
            document,
            "relationship-bundle",
            "<p class=\"memory-empty\">Provide a platform for memory relationships.</p>",
        );
    }
    let mut relation = build_model(model);
    if relation.edges.is_empty() {
        return set_html(
            document,
            "relationship-bundle",
            "<p class=\"memory-empty\">No relationships match the current filters and measure.</p>",
        );
    }
    let omitted_edges = limit_edges(&mut relation.edges);
    position_arc(&mut relation.sources, PI * 0.58, PI * 1.42);
    position_arc(&mut relation.targets, -PI * 0.42, PI * 0.42);
    let source_groups = group_anchors(&relation.sources);
    let target_groups = group_anchors(&relation.targets);
    let totals = edge_totals(&relation);
    let html = relationship_markup(
        model,
        &relation,
        &source_groups,
        &target_groups,
        &totals,
        omitted_edges,
    );
    set_html(document, "relationship-bundle", &html)?;
    draw_edges(model, document, &relation, &source_groups, &target_groups)
}

fn build_model(model: &AppModel) -> RelationModel {
    match model.state.relationship_mode.as_str() {
        "memory" => layer_memory_model(model),
        "pe-memory" => pe_memory_model(model),
        "tensor-memory" => tensor_memory_model(model),
        "tensor-pe" => tensor_pe_model(model),
        _ => compute_model(model),
    }
}

fn compute_model(model: &AppModel) -> RelationModel {
    let mut sources = Vec::new();
    let mut targets = HashMap::new();
    let mut edges = Vec::new();
    let (layers, omitted_sources) = relationship_layers(model);
    for layer in layers {
        sources.push(node(&layer.name, &layer.name, layer_band(&layer.name)));
        for layer_pe in layer
            .pes
            .iter()
            .filter(|pe| model.state.pes.is_selected(&pe.name))
        {
            let value = compute_edge_value(layer_pe, &model.state.relationship_measure);
            if value <= 0.0 {
                continue;
            }
            let pe = model.pe(&layer_pe.name);
            let row = pe.map_or(0, |pe| pe.row);
            targets
                .entry(layer_pe.name.clone())
                .or_insert_with(|| node(&layer_pe.name, &layer_pe.name, format!("PE row {row}")));
            edges.push(Edge {
                source: layer.name.clone(),
                target: layer_pe.name.clone(),
                value,
            });
        }
    }
    let mut targets = targets.into_values().collect::<Vec<_>>();
    sort_pes(model, &mut targets);
    RelationModel {
        sources,
        targets,
        edges,
        source_label: "layers",
        target_label: "PEs",
        omitted_sources,
    }
}

fn layer_memory_model(model: &AppModel) -> RelationModel {
    let memories = relationship_memories(model);
    let mut sources = Vec::new();
    let mut edges = Vec::new();
    let (layers, omitted_sources) = relationship_layers(model);
    for layer in layers {
        sources.push(node(&layer.name, &layer.name, layer_band(&layer.name)));
        let tensors = tensors_for_context(model, Some(&layer.name), None);
        edges.extend(memory_edges(
            model,
            &layer.name,
            &tensors,
            &memories,
            Some(&layer.name),
            None,
        ));
    }
    RelationModel {
        sources,
        targets: memory_targets(model, &memories),
        edges,
        source_label: "layers",
        target_label: "memories",
        omitted_sources,
    }
}

fn pe_memory_model(model: &AppModel) -> RelationModel {
    let memories = relationship_memories(model);
    let mut pes = model.compute_population();
    pes.sort_by_key(|pe| (pe.row, pe.col, pe.name.as_str()));
    let sources = pes
        .iter()
        .map(|pe| node(&pe.name, &pe.name, format!("PE row {}", pe.row)))
        .collect();
    let mut edges = Vec::new();
    for pe in pes {
        let tensors = tensors_for_context(model, None, Some(&pe.name));
        edges.extend(memory_edges(
            model,
            &pe.name,
            &tensors,
            &memories,
            None,
            Some(&pe.name),
        ));
    }
    RelationModel {
        sources,
        targets: memory_targets(model, &memories),
        edges,
        source_label: "PEs",
        target_label: "memories",
        omitted_sources: 0,
    }
}

fn tensor_memory_model(model: &AppModel) -> RelationModel {
    let memories = relationship_memories(model);
    let mut sources = Vec::new();
    let mut edges = Vec::new();
    for tensor in tensors_for_context(model, None, None) {
        let tensor_edges = memory_edges(model, &tensor.id, &[tensor], &memories, None, None);
        if !tensor_edges.is_empty() {
            sources.push(tensor_node(model, tensor));
            edges.extend(tensor_edges);
        }
    }
    sort_tensors(model, &mut sources);
    RelationModel {
        sources,
        targets: memory_targets(model, &memories),
        edges,
        source_label: "tensors",
        target_label: "memories",
        omitted_sources: 0,
    }
}

fn tensor_pe_model(model: &AppModel) -> RelationModel {
    let mut sources = Vec::new();
    let mut targets = HashMap::new();
    let mut edges = Vec::new();
    let use_reads = model.state.relationship_measure == "read";
    for tensor in tensors_for_context(model, None, None) {
        let traffic = model.tensor_traffic(tensor);
        let connections = if use_reads {
            &traffic.reads
        } else {
            &traffic.writes
        };
        if connections.is_empty() {
            continue;
        }
        sources.push(tensor_node(model, tensor));
        for connection in connections {
            let row = model.pe(&connection.pe).map_or(0, |pe| pe.row);
            targets
                .entry(connection.pe.clone())
                .or_insert_with(|| node(&connection.pe, &connection.pe, format!("PE row {row}")));
            edges.push(Edge {
                source: tensor.id.clone(),
                target: connection.pe.clone(),
                value: connection.bytes as f64,
            });
        }
    }
    let mut targets = targets.into_values().collect::<Vec<_>>();
    sort_pes(model, &mut targets);
    sort_tensors(model, &mut sources);
    RelationModel {
        sources,
        targets,
        edges,
        source_label: "tensors",
        target_label: "PEs",
        omitted_sources: 0,
    }
}

fn relationship_layers(model: &AppModel) -> (Vec<&crate::model::LayerSummary>, usize) {
    let all = model.filtered_layers().collect::<Vec<_>>();
    if all.len() <= MAX_LAYER_SOURCES {
        return (all, 0);
    }
    let mut visible = all[..MAX_LAYER_SOURCES].to_vec();
    let selected = model.state.selected_layer.as_deref();
    if let Some(layer) = all
        .iter()
        .find(|layer| Some(layer.name.as_str()) == selected)
        .filter(|layer| !visible.iter().any(|visible| visible.name == layer.name))
    {
        visible[MAX_LAYER_SOURCES - 1] = layer;
    }
    (visible, all.len() - MAX_LAYER_SOURCES)
}

fn limit_edges(edges: &mut Vec<Edge>) -> usize {
    let omitted = edges.len().saturating_sub(MAX_RENDERED_EDGES);
    if omitted == 0 {
        return 0;
    }
    edges.sort_by(|left, right| {
        right
            .value
            .total_cmp(&left.value)
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.target.cmp(&right.target))
    });
    edges.truncate(MAX_RENDERED_EDGES);
    omitted
}

fn memory_edges(
    model: &AppModel,
    source: &str,
    tensors: &[&TensorSummary],
    memories: &[&MemoryDeviceSummary],
    exact_layer: Option<&str>,
    exact_pe: Option<&str>,
) -> Vec<Edge> {
    let mut values: HashMap<String, f64> = HashMap::new();
    for tensor in tensors {
        for memory in memories {
            let traffic =
                model.tensor_traffic_for(tensor, exact_layer, exact_pe, Some(&memory.name));
            let traffic_bytes = if model.state.relationship_measure == "read" {
                traffic.read_bytes
            } else {
                traffic.write_bytes
            };
            *values.entry(memory.name.clone()).or_default() += traffic_bytes as f64;
        }
    }
    values
        .into_iter()
        .filter(|(_, value)| *value > 0.0)
        .map(|(target, value)| Edge {
            source: source.to_string(),
            target,
            value,
        })
        .collect()
}

fn tensors_for_context<'a>(
    model: &'a AppModel,
    exact_layer: Option<&str>,
    exact_pe: Option<&str>,
) -> Vec<&'a TensorSummary> {
    model
        .data
        .tensors
        .iter()
        .filter(|tensor| model.state.tensors.is_selected(&tensor.id))
        .filter(|tensor| model.tensor_memory_share(tensor) > 0.0)
        .filter(|tensor| {
            let traffic = model.tensor_traffic_for(tensor, exact_layer, exact_pe, None);
            traffic.edges > 0 || traffic.read_bytes > 0 || traffic.write_bytes > 0
        })
        .collect()
}

fn relationship_memories(model: &AppModel) -> Vec<&MemoryDeviceSummary> {
    model
        .data
        .memory
        .platform_memories
        .iter()
        .filter(|memory| model.state.memories.is_selected(&memory.name))
        .collect()
}

fn memory_targets(model: &AppModel, memories: &[&MemoryDeviceSummary]) -> Vec<Node> {
    memories
        .iter()
        .map(|memory| {
            let index = model
                .data
                .memory
                .platform_memories
                .iter()
                .position(|candidate| candidate.name == memory.name)
                .unwrap_or(0);
            let start = index / 4 * 4;
            node(
                &memory.name,
                &memory.name,
                format!("{} {start}-{}", memory.kind, start + 3),
            )
        })
        .collect()
}

fn relationship_markup(
    model: &AppModel,
    relation: &RelationModel,
    source_groups: &BTreeMap<String, Group>,
    target_groups: &BTreeMap<String, Group>,
    totals: &EdgeTotals,
    omitted_edges: usize,
) -> String {
    let source_label = singular(relation.source_label, relation.sources.len());
    let target_label = singular(relation.target_label, relation.targets.len());
    let mut svg = format!(
        "<svg viewBox=\"0 0 {WIDTH} {HEIGHT}\" role=\"group\" aria-label=\"{} bundled relationships between {} {} and {} {}\"><title>Hierarchical edge bundle of timetable relationships</title><g class=\"relationship-hierarchy\">",
        relation.edges.len(),
        relation.sources.len(),
        source_label,
        relation.targets.len(),
        target_label,
    );
    append_hierarchy(&mut svg, &relation.sources, source_groups);
    append_hierarchy(&mut svg, &relation.targets, target_groups);
    svg.push_str("</g>");
    append_nodes(
        &mut svg,
        model,
        &relation.sources,
        "source",
        &totals.sources,
        totals.maximum_source,
    );
    append_nodes(
        &mut svg,
        model,
        &relation.targets,
        "target",
        &totals.targets,
        totals.maximum_target,
    );
    svg.push_str("<g class=\"relationship-group-labels\">");
    for (name, group) in source_groups.iter().chain(target_groups) {
        write!(
            svg,
            "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>",
            group.point.x,
            group.point.y - 6.0,
            escape(name)
        )
        .unwrap();
    }
    svg.push_str("</g></svg>");
    let mode = model.state.relationship_measure.as_str();
    let total = if model.state.relationship_mode == "compute" {
        count(totals.total)
    } else {
        bytes(totals.total)
    };
    let window_status = relationship_window_status(relation.omitted_sources, omitted_edges);
    format!(
        "<div class=\"relationship-plot\"><canvas id=\"relationship-canvas\" width=\"1000\" height=\"620\" aria-hidden=\"true\"></canvas>{svg}</div><div class=\"relationship-status\"><span><i class=\"{}\"></i>{}</span><span>{} links</span><span>{} {}</span><span>{} {}</span><strong>{total} total</strong>{window_status}</div>",
        escape(mode),
        escape(measure_label(model)),
        integer(relation.edges.len() as u64),
        integer(relation.sources.len() as u64),
        escape(source_label),
        integer(relation.targets.len() as u64),
        escape(target_label),
    )
}

fn relationship_window_status(omitted_sources: usize, omitted_edges: usize) -> String {
    if omitted_sources == 0 && omitted_edges == 0 {
        return String::new();
    }
    format!(
        "<span class=\"filter-status\">Windowed: {} sources and {} links omitted; narrow filters to display them.</span>",
        integer(omitted_sources as u64),
        integer(omitted_edges as u64),
    )
}

fn append_hierarchy(html: &mut String, nodes: &[Node], groups: &BTreeMap<String, Group>) {
    for node in nodes {
        let group = &groups[&node.group];
        write!(
            html,
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"></line>",
            node.x, node.y, group.point.x, group.point.y
        )
        .unwrap();
    }
    for group in groups.values() {
        write!(
            html,
            "<line x1=\"{}\" y1=\"{}\" x2=\"{CENTER_X}\" y2=\"{CENTER_Y}\"></line>",
            group.point.x, group.point.y
        )
        .unwrap();
    }
}

fn append_nodes(
    html: &mut String,
    model: &AppModel,
    nodes: &[Node],
    side: &str,
    totals: &HashMap<String, f64>,
    maximum: f64,
) {
    write!(html, "<g class=\"relationship-nodes {side}\">").unwrap();
    let stride = nodes
        .len()
        .div_ceil(if side == "source" { 28 } else { 24 })
        .max(1);
    let selected = selected_id(model, side);
    let kind = entity_kind(model, side);
    let mode = model.state.relationship_measure.as_str();
    for (index, node) in nodes.iter().enumerate() {
        let value = totals.get(&node.id).copied().unwrap_or(0.0);
        let radius = 2.5 + (value / maximum.max(1.0)).sqrt() * 5.0;
        let is_selected = selected == Some(node.id.as_str());
        let formatted = if model.state.relationship_mode == "compute" {
            count(value)
        } else {
            bytes(value)
        };
        write!(
            html,
            "<circle cx=\"{}\" cy=\"{}\" r=\"{radius}\" class=\"{} weighted{} interactive\" role=\"button\" tabindex=\"0\" aria-label=\"Select {} {}\" aria-pressed=\"{}\" data-relationship-side=\"{side}\" data-relationship-kind=\"{}\" data-relationship-id=\"{}\"><title>{}: {formatted}; click to select, double-click to filter</title></circle>",
            node.x,
            node.y,
            escape(mode),
            is_selected.then_some(" selected").unwrap_or(""),
            escape(kind),
            escape(&node.label),
            is_selected,
            escape(kind),
            escape(&node.id),
            escape(&node.label),
        )
        .unwrap();
        if is_selected || nodes.len() <= 32 || index.is_multiple_of(stride) {
            let x = CENTER_X + node.angle.cos() * (LEAF_RADIUS + 12.0);
            let y = CENTER_Y + node.angle.sin() * (LEAF_RADIUS + 12.0);
            write!(
                html,
                "<text x=\"{x}\" y=\"{y}\" text-anchor=\"{}\" class=\"{} interactive\" data-relationship-side=\"{side}\" data-relationship-kind=\"{}\" data-relationship-id=\"{}\">{}</text>",
                if x < CENTER_X { "end" } else { "start" },
                is_selected.then_some("selected").unwrap_or(""),
                escape(kind),
                escape(&node.id),
                escape(&node.label),
            )
            .unwrap();
        }
    }
    html.push_str("</g>");
}

fn draw_edges(
    model: &AppModel,
    document: &Document,
    relation: &RelationModel,
    source_groups: &BTreeMap<String, Group>,
    target_groups: &BTreeMap<String, Group>,
) -> Result<(), JsValue> {
    let canvas = document
        .get_element_by_id("relationship-canvas")
        .ok_or_else(|| JsValue::from_str("Missing relationship canvas"))?
        .dyn_into::<HtmlCanvasElement>()?;
    let context = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("Canvas 2D rendering is unavailable"))?
        .dyn_into::<CanvasRenderingContext2d>()?;
    let source_by_id = relation
        .sources
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let target_by_id = relation
        .targets
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let maximum = relation
        .edges
        .iter()
        .map(|edge| edge.value)
        .fold(1.0_f64, f64::max);
    let colour = edge_colour(model, document);
    context.set_stroke_style_str(&colour);
    for edge in &relation.edges {
        let source = source_by_id[edge.source.as_str()];
        let target = target_by_id[edge.target.as_str()];
        let points = [
            Point {
                x: source.x,
                y: source.y,
            },
            source_groups[&source.group].point,
            Point {
                x: CENTER_X - 28.0,
                y: CENTER_Y,
            },
            Point {
                x: CENTER_X + 28.0,
                y: CENTER_Y,
            },
            target_groups[&target.group].point,
            Point {
                x: target.x,
                y: target.y,
            },
        ];
        let weight = (edge.value / maximum).sqrt();
        context.set_global_alpha(edge_alpha(relation.edges.len(), weight));
        context.set_line_width(0.35 + weight * 1.8);
        draw_curve(
            &context,
            points,
            model.state.relationship_strength as f64 / 100.0,
        );
    }
    context.set_global_alpha(1.0);
    Ok(())
}

fn draw_curve(context: &CanvasRenderingContext2d, hierarchy: [Point; 6], strength: f64) {
    let points = interpolate_hierarchy(hierarchy, strength);
    context.begin_path();
    context.move_to(points[0].x, points[0].y);
    for index in 0..points.len() - 1 {
        let previous = points[index.saturating_sub(1)];
        let current = points[index];
        let next = points[index + 1];
        let following = points[(index + 2).min(points.len() - 1)];
        let (source_control, target_control) = bezier_controls(previous, current, next, following);
        context.bezier_curve_to(
            source_control.x,
            source_control.y,
            target_control.x,
            target_control.y,
            next.x,
            next.y,
        );
    }
    context.stroke();
}

struct EdgeTotals {
    sources: HashMap<String, f64>,
    targets: HashMap<String, f64>,
    maximum_source: f64,
    maximum_target: f64,
    total: f64,
}

fn edge_totals(relation: &RelationModel) -> EdgeTotals {
    let mut sources = HashMap::new();
    let mut targets = HashMap::new();
    let mut total = 0.0;
    for edge in &relation.edges {
        *sources.entry(edge.source.clone()).or_default() += edge.value;
        *targets.entry(edge.target.clone()).or_default() += edge.value;
        total += edge.value;
    }
    let maximum_source = sources.values().copied().fold(1.0_f64, f64::max);
    let maximum_target = targets.values().copied().fold(1.0_f64, f64::max);
    EdgeTotals {
        sources,
        targets,
        maximum_source,
        maximum_target,
        total,
    }
}

fn position_arc(nodes: &mut [Node], start: f64, end: f64) {
    let denominator = nodes.len().saturating_sub(1).max(1) as f64;
    let only = nodes.len() == 1;
    for (index, node) in nodes.iter_mut().enumerate() {
        node.angle = if only {
            (start + end) / 2.0
        } else {
            start + (end - start) * index as f64 / denominator
        };
        node.x = CENTER_X + node.angle.cos() * LEAF_RADIUS;
        node.y = CENTER_Y + node.angle.sin() * LEAF_RADIUS;
    }
}

fn group_anchors(nodes: &[Node]) -> BTreeMap<String, Group> {
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for (index, node) in nodes.iter().enumerate() {
        groups
            .entry(node.group.clone())
            .or_insert_with(|| Group {
                point: Point { x: 0.0, y: 0.0 },
                nodes: Vec::new(),
            })
            .nodes
            .push(index);
    }
    for group in groups.values_mut() {
        let angle = group
            .nodes
            .iter()
            .map(|index| nodes[*index].angle)
            .sum::<f64>()
            / group.nodes.len() as f64;
        group.point = Point {
            x: CENTER_X + angle.cos() * GROUP_RADIUS,
            y: CENTER_Y + angle.sin() * GROUP_RADIUS,
        };
    }
    groups
}

fn compute_edge_value(pe: &crate::model::LayerPeSummary, measure: &str) -> f64 {
    match measure {
        "nodes" => pe.compute_nodes as f64,
        "adds" => pe.machine_ops.adds as f64,
        "muls" => pe.machine_ops.muls as f64,
        "compares" => pe.machine_ops.compares as f64,
        _ => pe.machine_ops.total as f64,
    }
}

fn tensor_node(model: &AppModel, tensor: &TensorSummary) -> Node {
    let layer = first_tensor_layer(model, tensor).unwrap_or_else(|| "Unassigned tensors".into());
    node(&tensor.id, &tensor.id, layer)
}

fn first_tensor_layer(model: &AppModel, tensor: &TensorSummary) -> Option<String> {
    let order = |name: &String| {
        model
            .data
            .layers
            .iter()
            .position(|layer| &layer.name == name)
            .unwrap_or(usize::MAX)
    };
    let mut production = connection_layers(model, &tensor.production_by_pe);
    production.sort_by_key(order);
    if let Some(layer) = production.first() {
        return Some(layer.clone());
    }
    let mut consumption = connection_layers(model, &tensor.consumption_by_pe);
    consumption.sort_by_key(order);
    consumption.first().cloned()
}

fn connection_layers(
    model: &AppModel,
    connections: &[crate::model::TensorPeConsumption],
) -> Vec<String> {
    let mut values = connections
        .iter()
        .flat_map(|connection| connection.by_layer.keys())
        .filter(|layer| model.state.layers.is_selected(layer))
        .cloned()
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn sort_pes(model: &AppModel, nodes: &mut [Node]) {
    nodes.sort_by_key(|node| {
        let pe = model.pe(&node.id);
        (
            pe.map_or(0, |pe| pe.row),
            pe.map_or(0, |pe| pe.col),
            node.id.clone(),
        )
    });
}

fn sort_tensors(model: &AppModel, nodes: &mut [Node]) {
    nodes.sort_by_key(|node| {
        let group = model
            .data
            .layers
            .iter()
            .position(|layer| layer.name == node.group)
            .unwrap_or(usize::MAX);
        let address = model.tensor(&node.id).map_or(0, |tensor| tensor.addr);
        (group, address, node.id.clone())
    });
}

fn node(id: &str, label: &str, group: String) -> Node {
    Node {
        id: id.into(),
        label: label.into(),
        group,
        x: 0.0,
        y: 0.0,
        angle: 0.0,
    }
}

fn layer_band(name: &str) -> String {
    let digits = name
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    match digits.parse::<usize>() {
        Ok(number) if number > 0 => {
            let start = (number - 1) / 10 * 10 + 1;
            format!("Layers {start}-{}", start + 9)
        }
        _ if name == "pre-layer" => "Pre-layer".into(),
        _ => "Unassigned layers".into(),
    }
}

fn selected_id<'a>(model: &'a AppModel, side: &str) -> Option<&'a str> {
    match (model.state.relationship_mode.as_str(), side) {
        ("pe-memory", "source") | ("tensor-pe", "target") | ("compute", "target") => {
            model.state.selected_pe.as_deref()
        }
        ("tensor-memory", "source") | ("tensor-pe", "source") => {
            model.state.selected_tensor.as_deref()
        }
        ("memory" | "pe-memory" | "tensor-memory", "target") => {
            model.state.selected_memory.as_deref()
        }
        (_, "source") => model.state.selected_layer.as_deref(),
        _ => None,
    }
}

fn entity_kind<'a>(model: &'a AppModel, side: &str) -> &'a str {
    match (model.state.relationship_mode.as_str(), side) {
        ("pe-memory", "source") | ("tensor-pe", "target") | ("compute", "target") => "pe",
        ("tensor-memory", "source") | ("tensor-pe", "source") => "tensor",
        ("memory" | "pe-memory" | "tensor-memory", "target") => "memory",
        _ => "layer",
    }
}

fn measure_label(model: &AppModel) -> &str {
    match model.state.relationship_measure.as_str() {
        "machine-ops" => "Machine ops",
        "nodes" => "Compute nodes",
        "read" => "Read",
        "write" => "Written",
        other => model
            .data
            .machine_ops
            .iter()
            .find(|operation| operation.name == other)
            .map_or(other, |operation| operation.label.as_str()),
    }
}

fn edge_colour(model: &AppModel, document: &Document) -> String {
    let property = match model.state.relationship_measure.as_str() {
        "read" => "--read",
        "write" => "--write",
        _ => "--activity-strong",
    };
    let Some(root) = document.document_element() else {
        return "#6677cc".into();
    };
    web_sys::window()
        .and_then(|window| window.get_computed_style(&root).ok().flatten())
        .and_then(|styles| styles.get_property_value(property).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "#6677cc".into())
}

fn singular(label: &str, count: usize) -> &str {
    if count != 1 {
        return label;
    }
    match label {
        "layers" => "layer",
        "PEs" => "PE",
        "memories" => "memory",
        "tensors" => "tensor",
        value => value,
    }
}

fn requires_platform(mode: &str) -> bool {
    matches!(mode, "memory" | "pe-memory" | "tensor-memory")
}

#[cfg(test)]
mod tests {
    use super::{layer_band, singular};

    #[test]
    fn groups_numbered_layers_in_tens() {
        assert_eq!(layer_band("layer 13"), "Layers 11-20");
        assert_eq!(layer_band("pre-layer"), "Pre-layer");
    }

    #[test]
    fn relationship_labels_are_singular_for_one_node() {
        assert_eq!(singular("memories", 1), "memory");
        assert_eq!(singular("memories", 2), "memories");
    }
}
