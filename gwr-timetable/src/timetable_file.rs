// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Types that map directly to the YAML file contents

use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::task::{ComputeOp, MemoryOp};
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

    pub fn validate(&self, platform: &Rc<Platform>) -> SimResult {
        let mut errors = Vec::new();

        // Iterate over nodes and build up set of all Node IDs whilst
        // also checking that any defined PE IDs are valid
        let mut node_ids = HashSet::new();
        for node in &self.nodes {
            let (id, pe) = node.id_pe();

            if !node_ids.insert(id.to_string()) {
                errors.push(format!("Duplicate Node ID '{id}'"));
            }

            if let Some(node_pe_id) = &pe
                && platform.pe_idx_from_name(node_pe_id).is_err()
            {
                errors.push(format!("Node '{id}' contains invalid PE ID '{node_pe_id}'"));
            }
        }

        // Ensure that all node IDs on edges are valid
        for edge in &self.edges {
            let from_id = edge.from_node_id();
            let to_id = edge.to_node_id();

            if !node_ids.contains(from_id) {
                errors.push(format!(
                    "Edge contains invalid from Node ID '{}'",
                    edge.from
                ));
            }

            if !node_ids.contains(to_id) {
                errors.push(format!("Edge contains invalid to Node ID '{}'", edge.to));
            }
        }

        // TODO:
        // - check for cycles in graph

        if !errors.is_empty() {
            return sim_error!("Failed to validate graph:\n{}", errors.join("\n"));
        }
        Ok(())
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
    #[serde(rename = "memory")]
    Memory {
        id: String,
        op: MemoryOp,
        pe: Option<String>,
        config: MemoryConfigSection,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryConfigSection {
    pub view: Option<TensorViewSection>,
}

/// Assuming best-case packing, how many bytes would num_elements of the given
/// dtype consume
#[must_use]
pub fn dtype_num_bytes(dtype: &DataType, num_elements: usize) -> usize {
    (dtype.num_bits() * num_elements).div_ceil(8)
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
            NodeSection::Memory { id, .. } => id,
            NodeSection::Tensor { id, .. } => id,
        }
    }

    #[must_use]
    pub fn id_pe(&self) -> (&String, &Option<String>) {
        match self {
            NodeSection::Compute { id, pe, .. } => (id, pe),
            NodeSection::Memory { id, pe, .. } => (id, pe),
            NodeSection::Tensor { id, .. } => (id, &None),
        }
    }

    #[must_use]
    pub fn pe(&self) -> &Option<String> {
        match self {
            NodeSection::Compute { pe, .. } => pe,
            NodeSection::Memory { pe, .. } => pe,
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
    if parts.len() == 2 {
        let index = match parts[1].parse::<usize>() {
            Ok(index) => Some(index),
            Err(e) => {
                return sim_error!("Unable to parse edge id '{id}'\n{e}");
            }
        };
        Ok((parts[0], index))
    } else {
        Ok((parts[0], None))
    }
}
