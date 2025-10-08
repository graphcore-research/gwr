// Copyright (c) 2025 Graphcore Ltd. All rights reserved.
//
use std::sync::Arc;

use tramway_models::ethernet_frame::{DEST_MAC_BYTES, EthernetFrame, u64_to_mac};
use tramway_track::entity::Entity;

/// A Packet Generator that can be used by the `Source` to produce packets on
/// the fly.
///
/// This allows each packet being created to be unique which aids debug of the
/// system.
pub struct PacketGen {
    pub entity: Arc<Entity>,
    dest: [u8; DEST_MAC_BYTES],
    payload_bytes: usize,
    num_send_packets: usize,
    num_sent_packets: usize,
}

impl PacketGen {
    #[must_use]
    pub fn new(
        parent: &Arc<Entity>,
        dest: [u8; DEST_MAC_BYTES],
        payload_bytes: usize,
        num_send_packets: usize,
    ) -> Self {
        Self {
            entity: Arc::new(Entity::new(parent, format!("gen{dest:?}").as_str())),
            dest,
            payload_bytes,
            num_send_packets,
            num_sent_packets: 0,
        }
    }
}

impl Iterator for PacketGen {
    type Item = EthernetFrame;
    fn next(&mut self) -> Option<Self::Item> {
        if self.num_sent_packets < self.num_send_packets {
            let label = self.num_sent_packets;
            self.num_sent_packets += 1;

            // Send to the correct `dest`, but set `src` to a unique value to aid debug
            // (packet count).
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
