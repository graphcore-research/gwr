// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Types that map directly to the YAML file contents

use std::collections::BTreeMap;
use std::path::Path;

use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::program::syntax::{
    AssignmentSpec, ProgramSpec, TemporarySpec, TileSpec,
};
use gwr_models::processing_element::tensors::dtype::DataType;
use serde::{Deserialize, Serialize};

use crate::graph::TimetableGraph;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimetableFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atomic_effects: Option<crate::atomic::AtomicGraphSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub programs: BTreeMap<String, ProgramDefinition>,
    pub nodes: Vec<NodeSection>,
    pub edges: Vec<EdgeSection>,
}

impl TimetableFile {
    pub fn from_file(graph_path: &Path) -> Result<Self, SimError> {
        // Generated JSON timetables already contain their complete catalog.
        // Deserialize them directly rather than building and serializing a
        // second YAML value tree for very large workloads. Files with includes
        // fall through to the normal catalog loader.
        let source = std::fs::read_to_string(graph_path).map_err(|error| {
            SimError(format!("Unable to read {}: {error}", graph_path.display()))
        })?;
        if source.trim_start().starts_with('{')
            && let Ok(file) = serde_json::from_str(&source)
        {
            return Ok(file);
        }
        let s = gwr_platform::catalog::load_yaml_with_includes(
            graph_path,
            gwr_platform::catalog::CatalogTarget::Timetable,
        )?;
        Self::from_string(&s)
    }

    pub fn from_string(graph_str: &str) -> Result<Self, SimError> {
        gwr_platform::catalog::reject_string_includes(graph_str)?;
        yaml_serde::from_str(graph_str)
            .map_err(|e| SimError(format!("yaml_serde::from_str failed: {e}")))
    }

    /// Validate graph structure, node connections, and tensor views.
    ///
    /// # Errors
    ///
    /// Returns an error when node IDs, edge endpoints, connections, or tensor
    /// views are invalid.
    pub fn validate(&self) -> SimResult {
        self.clone().into_graph().map(drop)
    }

    pub fn into_graph(self) -> Result<TimetableGraph, SimError> {
        TimetableGraph::build(self)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "kind")]
pub enum NodeSection {
    #[serde(rename = "compute")]
    Compute {
        id: String,
        program: ProgramSection,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tiling: Option<TileSpec>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pipeline: Option<gwr_models::processing_element::program::pipeline::BlockPipelineSpec>,
        pe: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_views: Option<Vec<Option<TensorViewSection>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_views: Option<Vec<Option<TensorViewSection>>>,
    },
    /// A bounded, byte-preserving copy through the issuing PE's memory path.
    #[serde(rename = "transfer")]
    Transfer {
        id: String,
        pe: Option<String>,
        #[serde(default = "default_transfer_buffer")]
        buffer_bytes: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_views: Option<Vec<Option<TensorViewSection>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_views: Option<Vec<Option<TensorViewSection>>>,
    },
    #[serde(rename = "tensor")]
    Tensor {
        id: String,
        config: TensorConfigSection,
    },
}

impl NodeSection {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            NodeSection::Compute { id, .. } | NodeSection::Transfer { id, .. } => id,
            NodeSection::Tensor { id, .. } => id,
        }
    }

    #[must_use]
    pub fn pe(&self) -> Option<&str> {
        match self {
            NodeSection::Compute { pe, .. } | NodeSection::Transfer { pe, .. } => pe.as_deref(),
            NodeSection::Tensor { .. } => None,
        }
    }
}

/// A compute node may define its program locally or use a shared definition.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ProgramSection {
    Named(String),
    Inline(Box<ProgramDefinition>),
}

impl From<ProgramSpec> for ProgramSection {
    fn from(program: ProgramSpec) -> Self {
        Self::Inline(Box::new(program.into()))
    }
}

/// A reusable program, optionally assembled from other named programs.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramDefinition {
    pub inputs: Vec<String>,
    /// Full-view boundary metadata loaded before arithmetic in each output
    /// tile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metadata_inputs: Vec<String>,
    /// Optional scalar Uint8 scale-plane range union; full backing/SRAM
    /// unchanged.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "gwr_models::processing_element::program::syntax::deserialize_indexed_metadata_inputs"
    )]
    pub indexed_metadata_inputs: Vec<String>,
    /// Exact source scalar local-byte frontier; composed/remapped use is
    /// rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_round_policy:
        Option<gwr_models::processing_element::program::syntax::LocalRoundPolicy>,
    pub outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub temporaries: BTreeMap<String, TemporarySpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignments: Vec<AssignmentSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<ProgramCall>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extents: BTreeMap<String, usize>,
}

impl From<ProgramSpec> for ProgramDefinition {
    fn from(program: ProgramSpec) -> Self {
        Self {
            inputs: program.inputs,
            metadata_inputs: program.metadata_inputs,
            indexed_metadata_inputs: program.indexed_metadata_inputs,
            local_round_policy: program.local_round_policy,
            outputs: program.outputs,
            temporaries: program.temporaries,
            assignments: program.assignments,
            calls: Vec::new(),
            extents: program.extents,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramCall {
    pub program: String,
    pub inputs: BTreeMap<String, String>,
    pub outputs: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub axes: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TensorViewSection {
    pub offsets: Vec<usize>,
    pub shape: Vec<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TensorConfigSection {
    #[serde(deserialize_with = "gwr_platform::types::parse_u64_byte_str")]
    pub addr: u64,
    pub dtype: DataType,
    pub shape: Vec<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeSection {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

impl EdgeSection {
    /// Return the node ID in the edge from end
    ///
    /// Endpoint IDs are `<node_id>[.<edge_index>]`. Node IDs may contain dots;
    /// only a final decimal token is interpreted as a port index.
    #[must_use]
    pub fn from_node_id(&self) -> &str {
        split_edge_end(&self.from).0
    }

    /// Return the node ID in the edge to end
    ///
    /// See `from_node_id` for more details.
    #[must_use]
    pub fn to_node_id(&self) -> &str {
        split_edge_end(&self.to).0
    }

    pub fn from_node_and_edge(&self) -> Result<(&str, Option<usize>), SimError> {
        parse_edge_end(&self.from)
    }

    pub fn to_node_and_edge(&self) -> Result<(&str, Option<usize>), SimError> {
        parse_edge_end(&self.to)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    /// Transfers tensor data and creates a scheduling dependency.
    Data,
    /// Records a graph relationship without affecting task readiness.
    Control,
    /// Enforces completion ordering without binding a tensor port.
    Dependency,
}

/// Take the string defining the end of an edge and return the index of
/// the node it corresponds to and the optional edge index in/out of that node.
///
/// For example:
///   gemm_0.1
/// will find the node named `gemm_0` defined in `node_idx_by_id` and return
/// Some(1) as the edge index into that node.
pub(crate) fn parse_edge_end(id: &str) -> Result<(&str, Option<usize>), SimError> {
    let (node_id, suffix) = split_edge_end(id);
    let index = suffix
        .map(|index| {
            index
                .parse::<usize>()
                .map_err(|error| SimError(format!("Unable to parse edge id '{id}': {error}")))
        })
        .transpose()?;
    Ok((node_id, index))
}

// Dots are permitted in logical node IDs. Only a final unsigned decimal token
// denotes a tensor port index; all other suffixes remain part of the node ID.
fn split_edge_end(id: &str) -> (&str, Option<&str>) {
    match id.rsplit_once('.') {
        Some((node, suffix))
            if !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            (node, Some(suffix))
        }
        _ => (id, None),
    }
}

fn default_transfer_buffer() -> usize {
    4096
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;
    #[test]
    fn dotted_node_ids_only_use_final_decimal_suffix_as_port() {
        for (endpoint, node, port) in [
            ("layer.0.dispatch", "layer.0.dispatch", None),
            ("layer.0.dispatch.12", "layer.0.dispatch", Some(12)),
            ("layer.name.tensor", "layer.name.tensor", None),
            ("node.0", "node", Some(0)),
        ] {
            let edge = EdgeSection {
                from: endpoint.into(),
                to: endpoint.into(),
                kind: EdgeKind::Data,
            };
            assert_eq!(edge.from_node_id(), node);
            assert_eq!(edge.to_node_id(), node);
            assert_eq!(edge.from_node_and_edge().unwrap(), (node, port));
            assert_eq!(edge.to_node_and_edge().unwrap(), (node, port));
        }
    }
}
