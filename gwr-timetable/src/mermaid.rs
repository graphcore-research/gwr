// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::hash::BuildHasher;

use gwr_models::processing_element::operators::shape_string;

use crate::timetable_file::{EdgeSection, NodeSection, TensorConfigSection, TensorViewSection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MermaidNodeStatus {
    Pending,
    Active,
    Complete,
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

fn tensor_label(id: &str, config: &TensorConfigSection) -> String {
    let dtype = &config.dtype;
    let shape = &config.shape;
    format!("{}\n{:?}\n{}", id, dtype, shape_string(shape))
}

fn create_view_string(prefix: &str, views: &[Option<TensorViewSection>]) -> String {
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
                    shape_string(&view.shape),
                    shape_string(&view.offsets)
                );
            }
        }
    }
    result
}

fn render_node_label(node: &NodeSection) -> String {
    match node {
        NodeSection::Tensor { id, config } => {
            format!("([{}])", escape_mermaid_label(&tensor_label(id, config)))
        }
        NodeSection::Compute {
            id: _,
            op,
            pe,
            input_views,
            output_views,
        } => {
            let pe = pe.as_deref().unwrap_or("?");
            let input_str = create_view_string("input", input_views);
            let output_str = create_view_string("output", output_views);
            format!(
                "[\"{}\"]",
                escape_mermaid_label(&format!(
                    "{input_str}\n{:?}\n{}\n{pe}\n\n{output_str}",
                    op,
                    node.id()
                ))
            )
        }
        NodeSection::Memory {
            id: _,
            op,
            pe: _,
            config,
        } => {
            let extra = match &config.view {
                Some(view) => {
                    let num_elements: usize = view.shape.iter().product();
                    format!(
                        "shape: {}\noffsets: {}\nelements: {num_elements}",
                        shape_string(&view.shape),
                        shape_string(&view.offsets)
                    )
                }
                None => "Full view".to_string(),
            };
            format!(
                "[\"{}\"]",
                escape_mermaid_label(&format!("{:?}\n{}\n{}", op, node.id(), extra))
            )
        }
    }
}

#[must_use]
pub fn render_mermaid_from_parts<T: BuildHasher>(
    nodes: &[NodeSection],
    edges: &[EdgeSection],
    statuses: &HashMap<String, MermaidNodeStatus, T>,
) -> String {
    let mut out = String::new();
    out.push_str("flowchart TD\n");

    for node in nodes {
        let _ = writeln!(
            out,
            "  {}{}",
            mermaid_id(node.id()),
            render_node_label(node)
        );
    }

    out.push_str("\n  %% Data-flow edges from timetable\n");
    for edge in edges {
        let _ = writeln!(
            out,
            "  {} -->|{:?}| {}",
            mermaid_id(edge.from_node_id()),
            edge.kind,
            mermaid_id(edge.to_node_id())
        );
    }

    out.push_str("\n  %% Styling\n");
    out.push_str("  classDef tensor fill:#eef7ff,stroke:#1f6feb,stroke-width:1px;\n");
    out.push_str("  classDef compute fill:#fff4e5,stroke:#9a6700,stroke-width:1px;\n");
    out.push_str("  classDef memory fill:#f6f8fa,stroke:#57606a,stroke-dasharray: 4 2;\n");
    out.push_str("  classDef tensorPending fill:#ffa0a0,stroke:#9a6700,stroke-width:2px;\n");
    out.push_str("  classDef tensorActive fill:#a0a0ff,stroke:#9a6700,stroke-width:4px;\n");
    out.push_str("  classDef tensorComplete fill:#a0ffa0,stroke:#9a6700,stroke-width:1px;\n");
    out.push_str("  classDef computePending fill:#ffa0a0,stroke:#9a6700,stroke-width:2px;\n");
    out.push_str("  classDef computeActive fill:#a0a0ff,stroke:#9a6700,stroke-width:4px;\n");
    out.push_str("  classDef computeComplete fill:#a0ffa0,stroke:#9a6700,stroke-width:1px;\n");

    let mut class_members: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for node in nodes {
        let class_name = match node {
            NodeSection::Tensor { id, .. } => match statuses.get(id) {
                Some(MermaidNodeStatus::Active) => "tensorActive",
                Some(MermaidNodeStatus::Complete) => "tensorComplete",
                Some(MermaidNodeStatus::Pending) => "tensorPending",
                None => "tensor",
            },
            NodeSection::Memory { .. } => "memory",
            NodeSection::Compute { id, .. } => match statuses.get(id) {
                Some(MermaidNodeStatus::Active) => "computeActive",
                Some(MermaidNodeStatus::Complete) => "computeComplete",
                Some(MermaidNodeStatus::Pending) => "computePending",
                None => "compute",
            },
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
