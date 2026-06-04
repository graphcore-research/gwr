// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::types::AccessType;
use gwr_model_builder::EntityGet;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::DeviceId;
use gwr_track::entity::Entity;

/// A frame Generator that can be used by the `Source` to produce frames on
/// the fly.
#[derive(EntityGet)]
pub struct FrameGen {
    entity: Rc<Entity>,
    payload_bytes: usize,
    overhead_bytes: usize,
    num_send_frames: usize,
    num_sent_frames: usize,
}

impl FrameGen {
    #[must_use]
    pub fn new(
        parent: &Rc<Entity>,
        overhead_bytes: usize,
        payload_bytes: usize,
        num_send_frames: usize,
    ) -> Self {
        Self {
            entity: Rc::new(Entity::new(parent, "frame_gen")),
            overhead_bytes,
            payload_bytes,
            num_send_frames,
            num_sent_frames: 0,
        }
    }
}

impl Iterator for FrameGen {
    type Item = MemoryAccess;
    fn next(&mut self) -> Option<Self::Item> {
        if self.num_sent_frames < self.num_send_frames {
            self.num_sent_frames += 1;

            Some(MemoryAccess::new(
                &self.entity,
                AccessType::WriteRequest,
                self.payload_bytes,
                self.num_sent_frames as u64,
                0,
                DeviceId(0),
                DeviceId(0),
                self.overhead_bytes,
            ))
        } else {
            None
        }
    }
}
