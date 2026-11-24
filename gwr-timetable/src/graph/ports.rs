// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;

use super::{EdgeDirection, TimetableNode};
use crate::timetable_file::{TensorViewSection, TensorViewSections, split_edge_end};

pub(super) struct ResolvedEndpoint<'a> {
    pub(super) node: usize,
    pub(super) index: Option<usize>,
    pub(super) name: Option<&'a str>,
}

pub(super) fn resolve_endpoint<'a>(
    endpoint: &'a str,
    indices: &HashMap<String, usize>,
    nodes: &[TimetableNode],
    direction: EdgeDirection,
) -> Result<ResolvedEndpoint<'a>, SimError> {
    // Preserve the original interpretation when a final decimal suffix
    // follows a known node ID. A declared numeric port takes precedence over
    // the legacy ordinal on that node.
    let (prefix, ordinal) = split_edge_end(endpoint);
    if let Some(ordinal) = ordinal
        && let Some(&node) = indices.get(prefix)
    {
        return resolve_port(node, ordinal, endpoint, nodes, direction);
    }
    if let Some(&node) = indices.get(endpoint) {
        return Ok(ResolvedEndpoint {
            node,
            index: None,
            name: None,
        });
    }
    // Match the longest declared node ID. The full remaining suffix is the
    // port name, since both node IDs and program ports may contain dots.
    for (separator, _) in endpoint.rmatch_indices('.') {
        if let Some(&node) = indices.get(&endpoint[..separator]) {
            return resolve_port(node, &endpoint[separator + 1..], endpoint, nodes, direction);
        }
    }
    if endpoint.is_empty() || endpoint.starts_with('.') {
        return invalid_endpoint(endpoint, direction, ": expected a nonempty node identifier");
    }
    invalid_endpoint(endpoint, direction, "")
}

pub(super) fn resolve_views(
    id: &str,
    direction: EdgeDirection,
    names: &[String],
    views: Option<TensorViewSections>,
) -> Result<Vec<Option<TensorViewSection>>, SimError> {
    match views {
        Some(TensorViewSections::Positional(views)) => Ok(views),
        Some(TensorViewSections::Named(mut views)) => {
            let ordered = names.iter().map(|name| views.remove(name)).collect();
            if let Some(name) = views.keys().next() {
                return sim_error!(
                    "Compute node '{id}' has unknown {direction} view port '{name}'"
                );
            }
            Ok(ordered)
        }
        None => Ok(vec![None; names.len()]),
    }
}

fn resolve_port<'a>(
    node: usize,
    port: &'a str,
    endpoint: &str,
    nodes: &[TimetableNode],
    direction: EdgeDirection,
) -> Result<ResolvedEndpoint<'a>, SimError> {
    if port.is_empty() {
        return invalid_endpoint(endpoint, direction, ": expected a nonempty port identifier");
    }
    let graph_node = &nodes[node];
    if let Some(program) = graph_node.program()
        && let Some(index) = direction
            .names(program)
            .iter()
            .position(|name| name == port)
    {
        return Ok(ResolvedEndpoint {
            node,
            index: Some(index),
            name: Some(port),
        });
    }
    if port.bytes().all(|byte| byte.is_ascii_digit()) {
        let index = port
            .parse::<usize>()
            .map_err(|error| SimError(format!("Unable to parse edge id '{endpoint}': {error}")))?;
        return Ok(ResolvedEndpoint {
            node,
            index: Some(index),
            name: None,
        });
    }
    if graph_node.is_compute() {
        return sim_error!(
            "Node '{}' has unknown {direction} port '{port}'",
            graph_node.id()
        );
    }
    invalid_endpoint(
        endpoint,
        direction,
        ": tensor and transfer nodes use positional endpoints",
    )
}

fn invalid_endpoint<T>(
    endpoint: &str,
    direction: EdgeDirection,
    details: &str,
) -> Result<T, SimError> {
    let end = match direction {
        EdgeDirection::Input => "to",
        EdgeDirection::Output => "from",
    };
    sim_error!("Edge contains invalid {end} Node ID '{endpoint}'{details}")
}
