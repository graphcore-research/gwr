// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Set of types used within the timetable that will wrap up any serializable /
//! deserializable types that are used directly in the YAML file.

use crate::timetable_file::NodeSection;

/// Wrapper structure to hold the Node section from the YAML file as well as any
/// other metadata like the structure of the TimeTable.
pub struct Node {
    pub node_section: NodeSection,
    pub inputs: Vec<Option<usize>>,
    pub outputs: Vec<Option<usize>>,
}
