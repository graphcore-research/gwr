// Copyright (c) 2025 Graphcore Ltd. All rights reserved.
//
use std::rc::Rc;

use tramway_models::data_frame::DataFrame;
use tramway_track::entity::Entity;

/// A frame Generator that can be used by the `Source` to produce frames on
/// the fly.
pub struct FrameGen {
    pub entity: Rc<Entity>,
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
    type Item = DataFrame;
    fn next(&mut self) -> Option<Self::Item> {
        if self.num_sent_frames < self.num_send_frames {
            self.num_sent_frames += 1;

            // Set `src` to a unique value to aid debug (frame count).
            Some(DataFrame::new(
                &self.entity,
                self.overhead_bytes,
                self.payload_bytes,
            ))
        } else {
            None
        }
    }
}
