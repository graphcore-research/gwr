// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashSet;

use gwr_models::processing_element::task::MemoryOp;

use crate::timetable_file::NodeSection;

/// Wrapper structure to hold the Node section from the YAML file as well as any
/// other metadata like the structure of the TimeTable.
pub struct Node {
    pub node_section: NodeSection,
    pub from_nodes: HashSet<usize>,
    pub to_nodes: HashSet<usize>,
}

impl Node {
    #[must_use]
    pub fn is_store_node(&self) -> bool {
        match self.node_section {
            NodeSection::Memory {
                id: _,
                op,
                pe: _,
                config: _,
            } => match op {
                MemoryOp::Store => true,
                MemoryOp::Load => false,
            },
            _ => false,
        }
    }

    #[must_use]
    pub fn get_tensor_node_idx(&self) -> Option<usize> {
        match self.node_section {
            NodeSection::Memory {
                id: _,
                op,
                pe: _,
                config: _,
            } => match op {
                MemoryOp::Load => {
                    if self.from_nodes.len() == 1 {
                        let from_node_idx = *self.from_nodes.iter().next().unwrap();
                        Some(from_node_idx)
                    } else {
                        None
                    }
                }
                MemoryOp::Store => {
                    if self.to_nodes.len() == 1 {
                        let to_node_idx = *self.to_nodes.iter().next().unwrap();
                        Some(to_node_idx)
                    } else {
                        None
                    }
                }
            },
            _ => None,
        }
    }
}
