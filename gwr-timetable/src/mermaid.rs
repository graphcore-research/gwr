// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::hash::BuildHasher;

use gwr_models::processing_element::operators::{HasShape, TensorView, shape_string};

use crate::graph::{TimetableGraph, TimetableNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MermaidNodeStatus {
    Pending,
    Active,
    Complete,
}

#[must_use]
pub fn render_mermaid<T: BuildHasher>(
    graph: &TimetableGraph,
    statuses: &HashMap<String, MermaidNodeStatus, T>,
) -> String {
    let mut out = String::new();
    out.push_str("flowchart TD\n");

    for (node_index, node) in graph.nodes().iter().enumerate() {
        let _ = writeln!(
            out,
            "  {}{}",
            mermaid_id(node.id()),
            render_node_label(graph, node_index, node)
        );
    }

    out.push_str("\n  %% Data-flow edges from timetable\n");
    for edge in graph.edges() {
        let _ = writeln!(
            out,
            "  {} -->|{:?}| {}",
            mermaid_id(graph.nodes()[edge.from().node()].id()),
            edge.kind(),
            mermaid_id(graph.nodes()[edge.to().node()].id())
        );
    }

    out.push_str("\n  %% Styling\n");
    out.push_str("  classDef tensor fill:#eef7ff,stroke:#1f6feb,stroke-width:1px;\n");
    out.push_str("  classDef compute fill:#fff4e5,stroke:#9a6700,stroke-width:1px;\n");
    out.push_str("  classDef tensorPending fill:#ffa0a0,stroke:#9a6700,stroke-width:2px;\n");
    out.push_str("  classDef tensorActive fill:#a0a0ff,stroke:#9a6700,stroke-width:4px;\n");
    out.push_str("  classDef tensorComplete fill:#a0ffa0,stroke:#9a6700,stroke-width:1px;\n");
    out.push_str("  classDef computePending fill:#ffa0a0,stroke:#9a6700,stroke-width:2px;\n");
    out.push_str("  classDef computeActive fill:#a0a0ff,stroke:#9a6700,stroke-width:4px;\n");
    out.push_str("  classDef computeComplete fill:#a0ffa0,stroke:#9a6700,stroke-width:1px;\n");

    let mut class_members: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for node in graph.nodes() {
        let class_name = if node.tensor().is_some() {
            match statuses.get(node.id()) {
                Some(MermaidNodeStatus::Active) => "tensorActive",
                Some(MermaidNodeStatus::Complete) => "tensorComplete",
                Some(MermaidNodeStatus::Pending) => "tensorPending",
                None => "tensor",
            }
        } else {
            match statuses.get(node.id()) {
                Some(MermaidNodeStatus::Active) => "computeActive",
                Some(MermaidNodeStatus::Complete) => "computeComplete",
                Some(MermaidNodeStatus::Pending) => "computePending",
                None => "compute",
            }
        };
        class_members
            .entry(class_name)
            .or_default()
            .push(mermaid_id(node.id()));
    }

    for (class_name, members) in class_members {
        let _ = writeln!(out, "  class {} {};", members.join(","), class_name);
    }

    out
}

fn mermaid_id(raw: &str) -> String {
    let mut s = String::from("n_");
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch);
        } else {
            s.push('_');
        }
    }
    s
}

fn escape_mermaid_label(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "<br/>")
}

fn create_view_string(prefix: &str, views: &[Option<TensorView>]) -> String {
    let mut result = String::new();
    for (idx, maybe_view) in views.iter().enumerate() {
        match maybe_view {
            None => {
                let _ = writeln!(result, "{prefix}{idx}: None");
            }
            Some(view) => {
                let _ = writeln!(
                    result,
                    "{prefix}{idx}: {} @ {}",
                    shape_string(view.shape().dims()),
                    shape_string(view.offsets())
                );
            }
        }
    }
    result
}

fn render_node_label(graph: &TimetableGraph, node_index: usize, node: &TimetableNode) -> String {
    if let Some(tensor) = node.tensor() {
        let label = format!(
            "{}\n{:?}\n{}",
            node.id(),
            tensor.dtype(),
            shape_string(tensor.shape().dims())
        );
        return format!("([{}])", escape_mermaid_label(&label));
    }

    let operation = node.operation().expect("non-tensor node has an operation");
    let views = graph
        .compute_views(node_index)
        .expect("non-tensor node has compute views");
    let pe = node.pe().unwrap_or("?");
    let input_str = create_view_string("input", views.inputs());
    let output_str = create_view_string("output", views.outputs());
    format!(
        "[\"{}\"]",
        escape_mermaid_label(&format!(
            "{input_str}\n{operation:?}\n{}\n{pe}\n\n{output_str}",
            node.id()
        ))
    )
}
