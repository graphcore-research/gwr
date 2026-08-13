// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Set of types used within the timetable that will wrap up any serializable /
//! deserializable types that are used directly in the YAML file.

use crate::timetable_file::NodeSection;

/// Wrapper structure to hold the Node section from the YAML file as well as any
/// other metadata like the structure of the TimeTable.
pub struct Node {
    pub node_section: NodeSection,
    /// Port-indexed tensor inputs connected by data edges.
    pub inputs: Vec<Option<usize>>,
    /// Port-indexed tensor outputs connected by data edges.
    pub outputs: Vec<Option<usize>>,
    /// Nodes that must complete before this node through data edges.
    pub predecessors: Vec<usize>,
    /// Nodes that depend on this node through data edges.
    pub successors: Vec<usize>,
}

impl Node {
    pub(crate) fn new(node_section: NodeSection) -> Self {
        Self {
            node_section,
            inputs: Vec::new(),
            outputs: Vec::new(),
            predecessors: Vec::new(),
            successors: Vec::new(),
        }
    }
}
