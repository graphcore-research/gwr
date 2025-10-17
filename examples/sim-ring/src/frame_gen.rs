// Copyright (c) 2025 Graphcore Ltd. All rights reserved.
//
use std::rc::Rc;

use gwr_models::ethernet_frame::{DEST_MAC_BYTES, EthernetFrame, u64_to_mac};
use gwr_track::entity::Entity;

/// A frame Generator that can be used by the `Source` to produce frames on
/// the fly.
///
/// This allows each frame being created to be unique which aids debug of the
/// system.
pub struct FrameGen {
    pub entity: Rc<Entity>,
    dest: [u8; DEST_MAC_BYTES],
    payload_bytes: usize,
    num_send_frames: usize,
    num_sent_frames: usize,
}

impl FrameGen {
    #[must_use]
    pub fn new(
        parent: &Rc<Entity>,
        dest: [u8; DEST_MAC_BYTES],
        payload_bytes: usize,
        num_send_frames: usize,
    ) -> Self {
        Self {
            entity: Rc::new(Entity::new(parent, format!("gen{dest:?}").as_str())),
            dest,
            payload_bytes,
            num_send_frames,
            num_sent_frames: 0,
        }
    }
}

impl Iterator for FrameGen {
    type Item = EthernetFrame;
    fn next(&mut self) -> Option<Self::Item> {
        if self.num_sent_frames < self.num_send_frames {
            let label = self.num_sent_frames;
            self.num_sent_frames += 1;

            // Send to the correct `dest`, but set `src` to a unique value to aid debug
            // (frame count).
            Some(
                EthernetFrame::new(&self.entity, self.payload_bytes)
                    .set_dest(self.dest)
                    .set_src(u64_to_mac(label as u64)),
            )
        } else {
            None
        }
    }
}
