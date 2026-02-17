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
pub struct Graph {
    pub nodes: Vec<NodeSection>,
    pub edges: Vec<EdgeSection>,
}

impl Graph {
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
                errors.push(format!("Duplicate node ID {id}"));
            }

            if let Some(node_pe_id) = &pe
                && platform.pe_idx_from_name(node_pe_id).is_err()
            {
                errors.push(format!("Node {id} contains invalid PE ID {node_pe_id}"));
            }
        }

        // Ensure that all node IDs on edges are valid
        for edge in &self.edges {
            if !node_ids.contains(&edge.from) {
                errors.push(format!("Edge contains invalid from Node ID {}", edge.from));
            }

            if !node_ids.contains(&edge.to) {
                errors.push(format!("Edge contains invalid to Node ID {}", edge.to));
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
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
pub enum NodeSection {
    #[serde(rename = "memory")]
    Memory {
        id: String,
        op: MemoryOp,
        pe: Option<String>,
        config: MemoryConfigSection,
    },
    #[serde(rename = "compute")]
    Compute {
        id: String,
        op: ComputeOp,
        pe: Option<String>,
        config: ComputeConfigSection,
    },
}

#[derive(Debug, Deserialize)]
pub struct MemoryConfigSection {
    #[serde(deserialize_with = "gwr_platform::types::parse_byte_str")]
    pub addr: u64,
    #[serde(deserialize_with = "gwr_platform::types::parse_byte_str")]
    pub num_bytes: u64,
}

#[derive(Debug, Deserialize)]
pub struct ComputeConfigSection {
    pub dtype: Option<String>,
    pub num_ops: Option<usize>,
}

impl NodeSection {
    #[must_use]
    pub fn id(&self) -> &String {
        match self {
            NodeSection::Memory {
                id,
                op: _,
                pe: _,
                config: _,
            } => id,
            NodeSection::Compute {
                id,
                op: _,
                pe: _,
                config: _,
            } => id,
        }
    }

    #[must_use]
    pub fn id_pe(&self) -> (&String, &Option<String>) {
        match self {
            NodeSection::Memory {
                id,
                op: _,
                pe,
                config: _,
            } => (id, pe),
            NodeSection::Compute {
                id,
                op: _,
                pe,
                config: _,
            } => (id, pe),
        }
    }

    #[must_use]
    pub fn pe(&self) -> &Option<String> {
        match self {
            NodeSection::Memory {
                id: _,
                op: _,
                pe,
                config: _,
            } => pe,
            NodeSection::Compute {
                id: _,
                op: _,
                pe,
                config: _,
            } => pe,
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
