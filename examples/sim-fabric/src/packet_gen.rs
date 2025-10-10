// Copyright (c) 2025 Graphcore Ltd. All rights reserved.
//
use std::rc::Rc;
use std::sync::Arc;

use rand::SeedableRng;
use rand::seq::{IteratorRandom, SliceRandom};
use rand_xoshiro::Xoshiro256PlusPlus;
use serde::Serialize;
use tramway_models::ethernet_frame::{EthernetFrame, u64_to_mac};
use tramway_models::fabric::FabricConfig;
use tramway_track::entity::Entity;

#[derive(clap::ValueEnum, Clone, Copy, Default, Debug, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum TrafficPattern {
    /// All sources will send to one dest chosen at random
    #[default]
    AllToOne,

    /// All sources will send to random (valid) destinations
    Random,

    /// All sources will be allocated a fixed (random) destination
    AllToAllFixed,

    /// All sources will send to to all other destinations in sequence
    AllToAllSeq,

    /// All sources will send to to all other destinations in sequence
    AllToAllRandom,
}

/// A Packet Generator that can be used by the `Source` to produce packets on
/// the fly.
///
/// This allows each packet being created to be unique which aids debug of the
/// system.
pub struct PacketGen {
    pub entity: Arc<Entity>,
    config: Rc<FabricConfig>,
    source_index: usize,
    dest_index: usize,
    traffic_pattern: TrafficPattern,
    payload_bytes: usize,
    num_send_packets: usize,
    num_sent_packets: usize,
    rng: Xoshiro256PlusPlus,
    next_dests: Vec<usize>,
}

impl PacketGen {
    #[expect(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        parent: &Arc<Entity>,
        config: Rc<FabricConfig>,
        source_index: usize,
        initial_dest_index: usize,
        traffic_pattern: TrafficPattern,
        payload_bytes: usize,
        num_send_packets: usize,
        seed: u64,
    ) -> Self {
        // Create a local RNG which is different per source
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(seed ^ (source_index as u64));
        let num_ports = config.num_ports();

        let dest_index = match traffic_pattern {
            TrafficPattern::Random => (0..num_ports).choose(&mut rng).unwrap(),
            TrafficPattern::AllToOne
            | TrafficPattern::AllToAllFixed
            | TrafficPattern::AllToAllSeq
            | TrafficPattern::AllToAllRandom => initial_dest_index,
        };

        let mut next_dests: Vec<usize> = (0..num_ports).collect();
        next_dests.shuffle(&mut rng);

        Self {
            entity: Arc::new(Entity::new(parent, format!("gen{source_index}").as_str())),
            config,
            source_index,
            dest_index,
            traffic_pattern,
            payload_bytes,
            num_send_packets,
            num_sent_packets: 0,
            rng,
            next_dests,
        }
    }
}

impl Iterator for PacketGen {
    type Item = EthernetFrame;
    fn next(&mut self) -> Option<Self::Item> {
        if self.num_sent_packets < self.num_send_packets {
            let mut label = u64_to_mac(self.num_sent_packets as u64);
            label[5] = self.source_index as u8;
            self.num_sent_packets += 1;

            // Send to the correct `dest`, but set `src` to a unique value to aid debug
            // (packet count).
            let frame = Some(
                EthernetFrame::new(&self.entity, self.payload_bytes)
                    .set_dest(u64_to_mac(self.dest_index as u64))
                    .set_src(label),
            );

            let num_ports = self.config.num_ports();
            self.dest_index = match self.traffic_pattern {
                TrafficPattern::Random => (0..num_ports).choose(&mut self.rng).unwrap(),
                TrafficPattern::AllToOne => self.dest_index,
                TrafficPattern::AllToAllFixed => self.dest_index,
                TrafficPattern::AllToAllSeq => (self.dest_index + 1) % num_ports,
                TrafficPattern::AllToAllRandom => {
                    let dest = self.next_dests.pop().unwrap();
                    if self.next_dests.is_empty() {
                        self.next_dests = (0..num_ports).collect();
                        self.next_dests.shuffle(&mut self.rng);
                    }
                    dest
                }
            };

            frame
        } else {
            None
        }
    }
}
