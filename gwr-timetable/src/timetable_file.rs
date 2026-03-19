// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Types that map directly to the JSON schema and file contents

use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::task::{ComputeOp, MemoryOp};
use gwr_platform::Platform;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
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
            if !node_ids.contains(&edge.from) {
                errors.push(format!(
                    "Edge contains invalid from Node ID '{}'",
                    edge.from
                ));
            }

            if !node_ids.contains(&edge.to) {
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Compute,
    Memory,
    Sync,
    Tensor,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
pub enum NodeSection {
    #[serde(rename = "compute")]
    Compute {
        id: String,
        op: ComputeOp,
        pe: Option<String>,
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

#[derive(Debug, Deserialize)]
pub struct MemoryConfigSection {
    #[serde(deserialize_with = "gwr_platform::types::parse_usize_byte_str")]
    pub offset: usize,
    #[serde(deserialize_with = "gwr_platform::types::parse_usize_byte_str")]
    pub num_elements: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataType {
    Fp32,
    Fp16,
    Bf16,
    Fp8,
    Fp4,
    Int32,
    Int16,
    Int8,
    Int4,
}

/// Return the number of bits for a DataType
#[must_use]
pub fn dtype_num_bits(dtype: &DataType) -> usize {
    match *dtype {
        DataType::Fp32 => 32,
        DataType::Fp16 => 16,
        DataType::Bf16 => 16,
        DataType::Fp8 => 8,
        DataType::Fp4 => 4,
        DataType::Int32 => 32,
        DataType::Int16 => 16,
        DataType::Int8 => 8,
        DataType::Int4 => 4,
    }
}

/// Assuming best-case packing, how many bytes would num_elements of the given
/// dtype consume
#[must_use]
pub fn dtype_num_bytes(dtype: &DataType, num_elements: usize) -> usize {
    (dtype_num_bits(dtype) * num_elements).div_ceil(8)
}

#[derive(Debug, Deserialize)]
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
        dtype_num_bits(&self.dtype)
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
            NodeSection::Compute { id, op: _, pe: _ } => id,
            NodeSection::Memory {
                id,
                op: _,
                pe: _,
                config: _,
            } => id,
            NodeSection::Tensor { id, config: _ } => id,
        }
    }

    #[must_use]
    pub fn id_pe(&self) -> (&String, &Option<String>) {
        match self {
            NodeSection::Compute { id, op: _, pe } => (id, pe),
            NodeSection::Memory {
                id,
                op: _,
                pe,
                config: _,
            } => (id, pe),
            NodeSection::Tensor { id, config: _ } => (id, &None),
        }
    }

    #[must_use]
    pub fn pe(&self) -> &Option<String> {
        match self {
            NodeSection::Compute { id: _, op: _, pe } => pe,
            NodeSection::Memory {
                id: _,
                op: _,
                pe,
                config: _,
            } => pe,
            NodeSection::Tensor { id: _, config: _ } => &None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    Data,
    Control,
}

#[derive(Debug, Deserialize)]
pub struct EdgeSection {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}
