// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::fmt;
use std::rc::Rc;

use gwr_models::data_frame::DataFrame;
use gwr_models::fabric::FabricConfig;
use gwr_track::entity::Entity;
use rand::SeedableRng;
use rand::seq::{IteratorRandom, SliceRandom};
use rand_xoshiro::Xoshiro256PlusPlus;
use serde::Serialize;

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

    /// All sources will send to to all other destinations in random (but
    /// repeated) order
    AllToAllRandom,
}

impl fmt::Display for TrafficPattern {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A frame Generator that can be used by the `Source` to produce frames on
/// the fly.
///
/// This allows each frame being created to be unique which aids debug of the
/// system.
pub struct FrameGen {
    pub entity: Rc<Entity>,
    config: Rc<FabricConfig>,
    source_index: usize,
    dest_index: usize,
    traffic_pattern: TrafficPattern,
    overhead_size_bytes: usize,
    payload_size_bytes: usize,
    num_send_frames: usize,
    num_sent_frames: usize,
    rng: Xoshiro256PlusPlus,
    next_dests: Vec<usize>,
}

impl FrameGen {
    #[expect(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        parent: &Rc<Entity>,
        config: Rc<FabricConfig>,
        source_index: usize,
        initial_dest_index: usize,
        traffic_pattern: TrafficPattern,
        overhead_size_bytes: usize,
        payload_size_bytes: usize,
        num_send_frames: usize,
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
            entity: Rc::new(Entity::new(parent, &format!("gen{source_index}"))),
            config,
            source_index,
            dest_index,
            traffic_pattern,
            overhead_size_bytes,
            payload_size_bytes,
            num_send_frames,
            num_sent_frames: 0,
            rng,
            next_dests,
        }
    }

    #[must_use]
    fn next_dest(&mut self) -> usize {
        let num_ports = self.config.max_num_ports();
        match self.traffic_pattern {
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
        }
    }
}

impl Iterator for FrameGen {
    type Item = DataFrame;
    fn next(&mut self) -> Option<Self::Item> {
        // If this can only send to self then there is nothing to do
        let no_valid_packets = (self.dest_index == self.source_index)
            && (self.traffic_pattern == TrafficPattern::AllToOne
                || self.traffic_pattern == TrafficPattern::AllToAllFixed);

        if no_valid_packets {
            return None;
        }

        if self.num_sent_frames < self.num_send_frames {
            while self.dest_index == self.source_index {
                self.dest_index = self.next_dest();
            }

            let label = (self.num_sent_frames | (self.source_index << 32)) as u64;
            self.num_sent_frames += 1;

            // Send to the correct `dest`, but set `src` to a unique value to aid debug
            // (frame count).
            let dest = self.config.port_indices()[self.dest_index];
            let frame = Some(
                DataFrame::new(
                    &self.entity,
                    self.overhead_size_bytes,
                    self.payload_size_bytes,
                )
                .set_dest(dest as u64)
                .set_label(label),
            );

            self.dest_index = self.next_dest();
            frame
        } else {
            None
        }
    }
}
