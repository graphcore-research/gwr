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
use super::super::relationships::{RelationshipEdge, RelationshipModel, RelationshipNode};
use super::super::state::{RelationshipMeasure, RelationshipMode};
use super::set_html;

const WIDTH: f64 = 1_000.0;
const HEIGHT: f64 = 620.0;
const CENTER_X: f64 = WIDTH / 2.0;
const CENTER_Y: f64 = HEIGHT / 2.0;
const LEAF_RADIUS: f64 = 250.0;
const GROUP_RADIUS: f64 = 132.0;
pub(super) fn render(model: &AppModel, document: &Document) -> Result<(), JsValue> {
    let strength = model.state.relationship_strength;
    document
        .get_element_by_id("relationship-strength-value")
        .ok_or_else(|| JsValue::from_str("Missing relationship strength output"))?
        .set_text_content(Some(&format!("{strength}%")));
    if model.state.relationship_mode.needs_platform()
        && model.data.memory.platform_memories.is_empty()
    {
        return set_html(
            document,
            "relationship-bundle",
            "<p class=\"memory-empty\">Provide a platform for memory relationships.</p>",
        );
    }
    let mut relation = rendered_relation(super::super::relationships::build(model));
    if relation.edges.is_empty() {
        return set_html(
            document,
            "relationship-bundle",
            "<p class=\"memory-empty\">No relationships match the current filters and measure.</p>",
        );
    }
    position_arc(&mut relation.sources, PI * 0.58, PI * 1.42);
    position_arc(&mut relation.targets, -PI * 0.42, PI * 0.42);
    let source_groups = group_anchors(&relation.sources);
    let target_groups = group_anchors(&relation.targets);
    let totals = edge_totals(&relation);
    let html = relationship_markup(model, &relation, &source_groups, &target_groups, &totals);
    set_html(document, "relationship-bundle", &html)?;
    draw_edges(model, document, &relation, &source_groups, &target_groups)
}

#[derive(Clone)]
struct Node {
    id: String,
    label: String,
    group: String,
    x: f64,
    y: f64,
    angle: f64,
}

struct RenderedRelationship {
    sources: Vec<Node>,
    targets: Vec<Node>,
    edges: Vec<RelationshipEdge>,
    source_label: &'static str,
    target_label: &'static str,
    total: f64,
    matching_edges: usize,
    omitted_sources: usize,
    omitted_edges: usize,
}

struct Group {
    point: Point,
    nodes: Vec<usize>,
}

fn rendered_relation(relation: RelationshipModel) -> RenderedRelationship {
    RenderedRelationship {
        sources: relation.sources.into_iter().map(rendered_node).collect(),
        targets: relation.targets.into_iter().map(rendered_node).collect(),
        edges: relation.edges,
        source_label: relation.source_label,
        target_label: relation.target_label,
        total: relation.total,
        matching_edges: relation.matching_edges,
        omitted_sources: relation.omitted_sources,
        omitted_edges: relation.omitted_edges,
    }
}

fn rendered_node(node: RelationshipNode) -> Node {
    Node {
        id: node.id,
        label: node.label,
        group: node.group,
        x: 0.0,
        y: 0.0,
        angle: 0.0,
    }
}

fn relationship_markup(
    model: &AppModel,
    relation: &RenderedRelationship,
    source_groups: &BTreeMap<String, Group>,
    target_groups: &BTreeMap<String, Group>,
    totals: &EdgeTotals,
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
    let mode = model.state.relationship_measure.name();
    let total = if model.state.relationship_mode == RelationshipMode::Compute {
        count(relation.total)
    } else {
        bytes(relation.total)
    };
    let window_status =
        relationship_window_status(relation.omitted_sources, relation.omitted_edges);
    format!(
        "<div class=\"relationship-plot\"><canvas id=\"relationship-canvas\" width=\"1000\" height=\"620\" aria-hidden=\"true\"></canvas>{svg}</div><div class=\"relationship-status\"><span><i class=\"{}\"></i>{}</span><span>{} links</span><span>{} {}</span><span>{} {}</span><strong>{total} total</strong>{window_status}</div>",
        escape(mode),
        escape(measure_label(model)),
        integer(relation.matching_edges as u64),
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
    let mode = model.state.relationship_measure.name();
    for (index, node) in nodes.iter().enumerate() {
        let value = totals.get(&node.id).copied().unwrap_or(0.0);
        let radius = 2.5 + (value / maximum.max(1.0)).sqrt() * 5.0;
        let is_selected = selected == Some(node.id.as_str());
        let formatted = if model.state.relationship_mode == RelationshipMode::Compute {
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
    relation: &RenderedRelationship,
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
}

fn edge_totals(relation: &RenderedRelationship) -> EdgeTotals {
    let mut sources = HashMap::new();
    let mut targets = HashMap::new();
    for edge in &relation.edges {
        *sources.entry(edge.source.clone()).or_default() += edge.value;
        *targets.entry(edge.target.clone()).or_default() += edge.value;
    }
    let maximum_source = sources.values().copied().fold(1.0_f64, f64::max);
    let maximum_target = targets.values().copied().fold(1.0_f64, f64::max);
    EdgeTotals {
        sources,
        targets,
        maximum_source,
        maximum_target,
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

fn selected_id<'a>(model: &'a AppModel, side: &str) -> Option<&'a str> {
    match (model.state.relationship_mode, side) {
        (RelationshipMode::PeMemory, "source")
        | (RelationshipMode::TensorPe, "target")
        | (RelationshipMode::Compute, "target") => model.state.selected_pe.as_deref(),
        (RelationshipMode::TensorMemory, "source") | (RelationshipMode::TensorPe, "source") => {
            model.state.selected_tensor.as_deref()
        }
        (
            RelationshipMode::LayerMemory
            | RelationshipMode::PeMemory
            | RelationshipMode::TensorMemory,
            "target",
        ) => model.state.selected_memory.as_deref(),
        (_, "source") => model.state.selected_layer.as_deref(),
        _ => None,
    }
}

fn entity_kind<'a>(model: &'a AppModel, side: &str) -> &'a str {
    match (model.state.relationship_mode, side) {
        (RelationshipMode::PeMemory, "source")
        | (RelationshipMode::TensorPe, "target")
        | (RelationshipMode::Compute, "target") => "pe",
        (RelationshipMode::TensorMemory, "source") | (RelationshipMode::TensorPe, "source") => {
            "tensor"
        }
        (
            RelationshipMode::LayerMemory
            | RelationshipMode::PeMemory
            | RelationshipMode::TensorMemory,
            "target",
        ) => "memory",
        _ => "layer",
    }
}

fn measure_label(model: &AppModel) -> &str {
    match &model.state.relationship_measure {
        RelationshipMeasure::MachineOps => "Machine ops",
        RelationshipMeasure::ComputeNodes => "Compute nodes",
        RelationshipMeasure::Read => "Read",
        RelationshipMeasure::Write => "Written",
        RelationshipMeasure::MachineOperation(name) => model
            .data
            .machine_ops
            .iter()
            .find(|operation| operation.name == *name)
            .map_or(name, |operation| &operation.label),
    }
}

fn edge_colour(model: &AppModel, document: &Document) -> String {
    let property = match model.state.relationship_measure {
        RelationshipMeasure::Read => "--read",
        RelationshipMeasure::Write => "--write",
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

#[cfg(test)]
mod tests {
    use super::singular;

    #[test]
    fn relationship_labels_are_singular_for_one_node() {
        assert_eq!(singular("memories", 1), "memory");
        assert_eq!(singular("memories", 2), "memories");
    }
}
