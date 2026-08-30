// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Types that map directly to the YAML file contents

use std::path::Path;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::task::ComputeOp;
use serde::{Deserialize, Serialize};

use crate::graph::TimetableGraph;

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TensorConfigSection {
    #[serde(deserialize_with = "gwr_platform::types::parse_u64_byte_str")]
    pub addr: u64,
    pub dtype: DataType,
    pub shape: Vec<usize>,
}

impl NodeSection {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            NodeSection::Compute { id, .. } => id,
            NodeSection::Tensor { id, .. } => id,
        }
    }

    #[must_use]
    pub fn pe(&self) -> Option<&str> {
        match self {
            NodeSection::Compute { pe, .. } => pe.as_deref(),
            NodeSection::Tensor { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    /// Transfers tensor data and creates a scheduling dependency.
    Data,
    /// Records a graph relationship without affecting task readiness.
    Control,
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
pub(crate) fn parse_edge_end(id: &str) -> Result<(&str, Option<usize>), SimError> {
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
