// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Set of types used within the timetable that will wrap up any serializable /
//! deserializable types that are used directly in the YAML file.

use gwr_models::processing_element::task::MemoryOp;

use crate::timetable_file::NodeSection;

/// Wrapper structure to hold the Node section from the YAML file as well as any
/// other metadata like the structure of the TimeTable.
pub struct Node {
    pub node_section: NodeSection,
    pub inputs: Vec<Option<usize>>,
    pub outputs: Vec<Option<usize>>,
}

impl Node {
    #[must_use]
    pub fn is_store_node(&self) -> bool {
        match self.node_section {
            NodeSection::Memory { op, .. } => match op {
                MemoryOp::Store => true,
                MemoryOp::Load => false,
            },
            _ => false,
        }
    }

    #[must_use]
    pub fn get_memory_tensor_node_idx(&self) -> Option<usize> {
        match self.node_section {
            NodeSection::Memory { op, .. } => match op {
                MemoryOp::Load => {
                    if self.inputs.len() == 1 {
                        *self.inputs.first().unwrap()
                    } else {
                        todo!()
                    }
                }
                MemoryOp::Store => {
                    if self.outputs.len() == 1 {
                        *self.outputs.first().unwrap()
                    } else {
                        todo!()
                    }
                }
            },
            _ => None,
        }
    }

    #[must_use]
    pub fn get_output_tensor_node_idx(&self) -> Option<usize> {
        if self.outputs.len() == 1 {
            *self.outputs.first().unwrap()
        } else {
            todo!()
        }
    }
}
