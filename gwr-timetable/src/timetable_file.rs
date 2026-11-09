// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Types that map directly to the YAML file contents

use std::collections::BTreeMap;
use std::fmt;
use std::fs::read_to_string;
use std::path::Path;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
use gwr_models::processing_element::tensors::dtype::DataType;
use serde::de::{Deserializer, Error as DeError, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use yaml_serde::from_str;

use crate::graph::TimetableGraph;

/// Serialized workload nodes and their named data or control edges.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimetableFile {
    /// Compute and tensor nodes in file order.
    pub nodes: Vec<NodeSection>,
    /// Data bindings and control relationships between nodes.
    pub edges: Vec<EdgeSection>,
}

impl TimetableFile {
    pub fn from_file(graph_path: &Path) -> Result<Self, SimError> {
        let s = read_to_string(graph_path)
            .map_err(|e| SimError(format!("Unable to read {}: {e}", graph_path.display())))?;
        Self::from_string(&s)
    }

    pub fn from_string(graph_str: &str) -> Result<Self, SimError> {
        from_str(graph_str).map_err(|e| SimError(format!("yaml_serde::from_str failed: {e}")))
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

    /// Resolve named ports and tensor views, then validate data dependencies.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid nodes, bindings, views or conflicting
    /// accesses.
    pub fn into_graph(self) -> Result<TimetableGraph, SimError> {
        TimetableGraph::build(self)
    }
}

/// A serialized compute program or a physical tensor.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "kind")]
pub enum NodeSection {
    /// A program with optional tiling, PE placement and named tensor views.
    #[serde(rename = "compute")]
    Compute {
        /// Unique node identifier, without the port delimiter.
        id: String,
        /// Program declarations and assignments to prepare for execution.
        program: Box<ProgramSpec>,
        /// Optional per-axis tile sizes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tiling: Option<TileSpec>,
        /// Name of the processing element selected for execution.
        pe: Option<String>,
        /// Sparse input views keyed by declared program input name.
        #[serde(
            default,
            skip_serializing_if = "BTreeMap::is_empty",
            deserialize_with = "deserialize_views"
        )]
        input_views: BTreeMap<String, TensorViewSection>,
        /// Sparse output views keyed by declared program output name.
        #[serde(
            default,
            skip_serializing_if = "BTreeMap::is_empty",
            deserialize_with = "deserialize_views"
        )]
        output_views: BTreeMap<String, TensorViewSection>,
    },
    /// A tensor's identity and physical storage descriptor.
    #[serde(rename = "tensor")]
    Tensor {
        /// Unique node identifier, without the port delimiter.
        id: String,
        /// Physical storage descriptor for this tensor.
        config: TensorConfigSection,
    },
}

impl NodeSection {
    /// Node identifier used by edge endpoints.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            NodeSection::Compute { id, .. } => id,
            NodeSection::Tensor { id, .. } => id,
        }
    }

    /// Selected PE name for compute nodes, or `None` for tensor nodes.
    #[must_use]
    pub fn pe(&self) -> Option<&str> {
        match self {
            NodeSection::Compute { pe, .. } => pe.as_deref(),
            NodeSection::Tensor { .. } => None,
        }
    }
}

/// Shape and offsets of a view bound to a declared compute port.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TensorViewSection {
    /// Starting coordinate of the view within its tensor.
    pub offsets: Vec<usize>,
    /// Number of selected elements along each dimension.
    pub shape: Vec<usize>,
}

/// Physical address, element type and dimensions of a tensor node.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TensorConfigSection {
    /// Physical byte address of the tensor storage.
    #[serde(deserialize_with = "gwr_platform::types::parse_u64_byte_str")]
    pub addr: u64,
    /// Element type stored in the tensor.
    pub dtype: DataType,
    /// Number of elements along each dimension.
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
    /// Return the source node ID (before the optional port delimiter).
    #[must_use]
    pub fn from_node_id(&self) -> &str {
        self.from.split('.').next().expect("split has a first item")
    }

    /// Return the destination node ID (before the optional port delimiter).
    #[must_use]
    pub fn to_node_id(&self) -> &str {
        self.to.split('.').next().expect("split has a first item")
    }

    /// Parse the source as a node ID and optional declared output port name.
    pub fn from_node_and_port(&self) -> Result<(&str, Option<&str>), SimError> {
        parse_edge_end(&self.from)
    }

    /// Parse the destination as a node ID and optional declared input port
    /// name.
    pub fn to_node_and_port(&self) -> Result<(&str, Option<&str>), SimError> {
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
}

/// Parse `node` or `node.PORT`, splitting only on the first dot.
/// The complete suffix is a declared name, never a numeric ordinal.
pub(crate) fn parse_edge_end(id: &str) -> Result<(&str, Option<&str>), SimError> {
    let (node, port) = id
        .split_once('.')
        .map_or((id, None), |(node, port)| (node, Some(port)));
    validate_node_id(node)?;
    if let Some(port) = port {
        validate_port_name(port)?;
    }
    Ok((node, port))
}

/// Node IDs must be nonempty and cannot contain the endpoint delimiter.
/// Port names are the entire nonempty suffix after the first dot.
pub(crate) fn validate_node_id(id: &str) -> Result<(), SimError> {
    if id.is_empty() || id.contains('.') {
        return sim_error!(
            "Invalid timetable node identifier '{id}': expected a nonempty name without '.'"
        );
    }
    Ok(())
}

pub(crate) fn validate_port_name(name: &str) -> Result<(), SimError> {
    if name.is_empty() {
        return sim_error!("Invalid timetable port identifier: expected a nonempty name");
    }
    Ok(())
}

fn deserialize_views<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, TensorViewSection>, D::Error>
where
    D: Deserializer<'de>,
{
    struct Views;

    impl<'de> Visitor<'de> for Views {
        type Value = BTreeMap<String, TensorViewSection>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a map of program port names to tensor views")
        }

        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut views = BTreeMap::new();
            while let Some((name, view)) = map.next_entry::<String, TensorViewSection>()? {
                if views.contains_key(&name) {
                    return Err(DeError::custom(format!("duplicate view port '{name}'")));
                }
                views.insert(name, view);
            }
            Ok(views)
        }
    }
    deserializer.deserialize_map(Views)
}
