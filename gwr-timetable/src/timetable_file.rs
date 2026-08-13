// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Types that map directly to the YAML file contents

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::task::ComputeOp;
use gwr_platform::Platform;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimetableFile {
    pub nodes: Vec<NodeSection>,
    pub edges: Vec<EdgeSection>,
}

impl TimetableFile {
    pub fn from_file(graph_path: &Path) -> Result<Self, SimError> {
        let s = std::fs::read_to_string(graph_path)
            .map_err(|e| SimError(format!("Unable to read {}: {e}", graph_path.display())))?;
        Self::from_string(&s)
    }

    pub fn from_string(graph_str: &str) -> Result<Self, SimError> {
        serde_yaml::from_str(graph_str)
            .map_err(|e| SimError(format!("serde_yaml::from_str failed: {e}")))
    }

    pub(crate) fn validate_structure(&self) -> SimResult {
        let mut errors = Vec::new();
        let mut nodes_by_id = HashMap::new();
        for node in &self.nodes {
            let id = node.id();
            if nodes_by_id.insert(id.to_string(), node).is_some() {
                errors.push(format!("Duplicate Node ID '{id}'"));
            }
        }

        let mut connected_inputs = HashMap::new();
        let mut connected_outputs = HashMap::new();
        let endpoint_counts = data_endpoint_counts(&self.edges);
        for edge in &self.edges {
            let track_port = matches!(&edge.kind, EdgeKind::Data);
            validate_edge_end(
                &edge.from,
                "from",
                "output",
                track_port,
                &nodes_by_id,
                &endpoint_counts,
                &mut connected_outputs,
                &mut errors,
            );
            validate_edge_end(
                &edge.to,
                "to",
                "input",
                track_port,
                &nodes_by_id,
                &endpoint_counts,
                &mut connected_inputs,
                &mut errors,
            );
        }

        if !errors.is_empty() {
            return sim_error!("Failed to validate graph:\n{}", errors.join("\n"));
        }
        Ok(())
    }

    /// Validate graph structure, node connections, and tensor views.
    ///
    /// # Errors
    ///
    /// Returns an error when node IDs, edge endpoints, connections, or tensor
    /// views are invalid.
    pub fn validate(&self) -> SimResult {
        crate::validation::validate_file(self)
    }

    pub(crate) fn validate_platform_references(&self, platform: &Rc<Platform>) -> SimResult {
        let errors = self
            .nodes
            .iter()
            .filter_map(|node| {
                let (id, pe) = node.id_pe();
                let pe = pe.as_ref()?;
                platform
                    .pe_idx_from_name(pe)
                    .is_err()
                    .then(|| format!("Node '{id}' contains invalid PE ID '{pe}'"))
            })
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            return sim_error!("Failed to validate graph:\n{}", errors.join("\n"));
        }
        Ok(())
    }
}

#[expect(clippy::too_many_arguments)]
fn validate_edge_end(
    endpoint: &str,
    direction: &str,
    port_kind: &str,
    track_port: bool,
    nodes_by_id: &HashMap<String, &NodeSection>,
    endpoint_counts: &HashMap<(String, &'static str), usize>,
    connected_ports: &mut HashMap<String, HashSet<usize>>,
    errors: &mut Vec<String>,
) {
    let Ok((node_id, port)) = parse_edge_end(endpoint).inspect_err(|error| {
        errors.push(error.to_string());
    }) else {
        return;
    };
    let Some(node) = nodes_by_id.get(node_id) else {
        errors.push(format!(
            "Edge contains invalid {direction} Node ID '{endpoint}'"
        ));
        return;
    };
    if !track_port {
        return;
    }
    let port_count = node_port_count(node, node_id, port_kind, endpoint_counts);
    let ports = connected_ports.entry(node_id.to_string()).or_default();
    let port = match port {
        Some(port) => port,
        None => next_implicit_port(ports, port_count),
    };
    if let Some(limit) = port_count
        && port >= limit
    {
        errors.push(format!(
            "Node '{node_id}' {port_kind} edge index {port} is out of range for {limit} declared {port_kind} ports"
        ));
        return;
    }
    if ports.contains(&port) {
        errors.push(format!(
            "Node '{node_id}' {port_kind} edge index {port} is connected more than once"
        ));
    } else {
        ports.insert(port);
    }
}

fn next_implicit_port(ports: &HashSet<usize>, port_count: Option<usize>) -> usize {
    let limit = port_count.unwrap_or_else(|| ports.len().saturating_add(1));
    (0..limit)
        .find(|candidate| !ports.contains(candidate))
        .unwrap_or(limit)
}

fn data_endpoint_counts(edges: &[EdgeSection]) -> HashMap<(String, &'static str), usize> {
    let mut counts = HashMap::new();
    for edge in edges {
        if !matches!(&edge.kind, EdgeKind::Data) {
            continue;
        }
        if let Ok((node_id, _)) = edge.from_node_and_edge() {
            *counts.entry((node_id.to_string(), "output")).or_default() += 1;
        }
        if let Ok((node_id, _)) = edge.to_node_and_edge() {
            *counts.entry((node_id.to_string(), "input")).or_default() += 1;
        }
    }
    counts
}

fn node_port_count(
    node: &NodeSection,
    node_id: &str,
    port_kind: &str,
    endpoint_counts: &HashMap<(String, &'static str), usize>,
) -> Option<usize> {
    match (node, port_kind) {
        (NodeSection::Compute { input_views, .. }, "input") => Some(input_views.len()),
        (NodeSection::Compute { output_views, .. }, "output") => Some(output_views.len()),
        (NodeSection::Tensor { .. }, _) => None,
        _ => endpoint_counts
            .get(&(node_id.to_string(), port_kind))
            .copied(),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "kind")]
pub enum NodeSection {
    #[serde(rename = "compute")]
    Compute {
        id: String,
        op: ComputeOp,
        pe: Option<String>,
        input_views: Vec<Option<TensorViewSection>>,
        output_views: Vec<Option<TensorViewSection>>,
    },
    #[serde(rename = "tensor")]
    Tensor {
        id: String,
        config: TensorConfigSection,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TensorViewSection {
    pub offsets: Vec<usize>,
    pub shape: Vec<usize>,
}

impl TensorViewSection {
    #[must_use]
    pub fn num_elements(&self) -> usize {
        self.shape.iter().product()
    }
}

/// Assuming best-case packing, how many bytes would num_elements of the given
/// dtype consume
#[must_use]
pub fn dtype_num_bytes(dtype: &DataType, num_elements: usize) -> usize {
    (dtype.num_bits() * num_elements).div_ceil(8)
}

/// Return the physical byte range touched by a contiguous view of
/// `num_elements` elements starting at `element_offset`.
#[must_use]
pub fn checked_dtype_byte_range(
    dtype: &DataType,
    element_offset: u64,
    num_elements: u64,
) -> Option<std::ops::Range<u64>> {
    let bits_per_element = u128::try_from(dtype.num_bits()).ok()?;
    let start_bit = u128::from(element_offset).checked_mul(bits_per_element)?;
    let num_bits = u128::from(num_elements).checked_mul(bits_per_element)?;
    let start_byte = u64::try_from(start_bit / 8).ok()?;
    if num_bits == 0 {
        return Some(start_byte..start_byte);
    }
    let end_bit = start_bit.checked_add(num_bits)?;
    let end_byte = u64::try_from(end_bit.div_ceil(8)).ok()?;
    Some(start_byte..end_byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_element_byte_range_is_empty_at_starting_byte() {
        assert_eq!(checked_dtype_byte_range(&DataType::Int4, 1, 0), Some(0..0));
        assert_eq!(
            checked_dtype_byte_range(&DataType::Int8, u64::MAX, 0),
            Some(u64::MAX..u64::MAX)
        );
    }

    #[test]
    fn byte_range_accepts_representable_large_offset() {
        let element_offset = 1_u64 << 61;

        assert_eq!(
            checked_dtype_byte_range(&DataType::Int8, element_offset, 1),
            Some(element_offset..(element_offset + 1))
        );
    }

    #[test]
    fn byte_range_rejects_unrepresentable_end() {
        assert_eq!(checked_dtype_byte_range(&DataType::Int8, u64::MAX, 1), None);
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TensorConfigSection {
    #[serde(deserialize_with = "gwr_platform::types::parse_u64_byte_str")]
    pub addr: u64,
    pub dtype: DataType,
    pub shape: Vec<usize>,
}

impl TensorConfigSection {
    /// Number of bits per elements defined by this tensor
    #[must_use]
    pub fn bits_per_element(&self) -> usize {
        self.dtype.num_bits()
    }

    /// Number of elements defined by this tensor
    #[must_use]
    pub fn num_elements(&self) -> usize {
        self.shape.iter().product()
    }

    /// Number of bytes defined by this tensor
    #[must_use]
    pub fn num_bytes(&self) -> usize {
        dtype_num_bytes(&self.dtype, self.num_elements())
    }
}

impl NodeSection {
    #[must_use]
    pub fn id(&self) -> &String {
        match self {
            NodeSection::Compute { id, .. } => id,
            NodeSection::Tensor { id, .. } => id,
        }
    }

    #[must_use]
    pub fn id_pe(&self) -> (&String, &Option<String>) {
        match self {
            NodeSection::Compute { id, pe, .. } => (id, pe),
            NodeSection::Tensor { id, .. } => (id, &None),
        }
    }

    #[must_use]
    pub fn pe(&self) -> &Option<String> {
        match self {
            NodeSection::Compute { pe, .. } => pe,
            NodeSection::Tensor { .. } => &None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    Data,
    Control,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeSection {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

impl EdgeSection {
    /// Return the node ID in the edge from end
    ///
    /// The string are of the form:
    ///   <node_id>[.<edge_index>?
    /// So we split on the '.' and return the first part
    #[must_use]
    pub fn from_node_id(&self) -> &str {
        let from: Vec<&str> = self.from.split('.').collect();
        from[0]
    }

    /// Return the node ID in the edge to end
    ///
    /// See `from_node_id` for more details.
    #[must_use]
    pub fn to_node_id(&self) -> &str {
        let to: Vec<&str> = self.to.split('.').collect();
        to[0]
    }

    pub fn from_node_and_edge(&self) -> Result<(&str, Option<usize>), SimError> {
        parse_edge_end(&self.from)
    }

    pub fn to_node_and_edge(&self) -> Result<(&str, Option<usize>), SimError> {
        parse_edge_end(&self.to)
    }
}

/// Take the string defining the end of an edge and return the index of
/// the node it corresponds to and the optional edge index in/out of that node.
///
/// For example:
///   gemm_0.1
/// will find the node named `gemm_0` defined in `node_idx_by_id` and return
/// Some(1) as the edge index into that node.
fn parse_edge_end(id: &str) -> Result<(&str, Option<usize>), SimError> {
    let parts: Vec<&str> = id.split('.').collect();
    match parts.as_slice() {
        [node_id] => Ok((node_id, None)),
        [node_id, edge_id] => {
            let index = edge_id
                .parse::<usize>()
                .map_err(|error| SimError(format!("Unable to parse edge id '{id}'\n{error}")))?;
            Ok((node_id, Some(index)))
        }
        _ => sim_error!("Unable to parse edge id '{id}'"),
    }
}
