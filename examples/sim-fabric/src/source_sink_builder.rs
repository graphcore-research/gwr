// Copyright (c) 2025 Graphcore Ltd. All rights reserved.
//
//! Library functions to build parts of the sim-fabric application.

use std::rc::Rc;

use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_engine::engine::Engine;
use gwr_models::ethernet_frame::EthernetFrame;
use gwr_models::fabric::FabricConfig;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_xoshiro::Xoshiro256PlusPlus;

use crate::frame_gen::{FrameGen, TrafficPattern};

// Define some types to aid readability
pub type Sources = Vec<Rc<Source<EthernetFrame>>>;
pub type Sinks = Vec<Rc<Sink<EthernetFrame>>>;

pub fn build_source_sinks(
    engine: &mut Engine,
    config: &Rc<FabricConfig>,
    traffic_pattern: TrafficPattern,
    packet_payload_bytes: usize,
    num_send_packets: usize,
    seed: u64,
    num_active_sources: usize,
) -> (Sources, Sinks) {
    let top = engine.top();

    let num_ports = config.num_ports();
    let mut sources = Vec::with_capacity(num_ports);

    let mut rng = Xoshiro256PlusPlus::seed_from_u64(seed);

    // Create random set of sources that will be active
    let mut all_port_indices: Vec<usize> = (0..num_ports).collect();
    all_port_indices.shuffle(&mut rng);
    let active_port_indices: Vec<usize> = all_port_indices
        .into_iter()
        .take(num_active_sources)
        .collect();

    // Create an random set of initial assigments
    let mut dest_indices: Vec<usize> = (0..num_ports).collect();
    dest_indices.shuffle(&mut rng);

    let first_dest = dest_indices[0];

    for (i, dest_index) in dest_indices.drain(..).enumerate() {
        let config = config.clone();
        let initial_dest_index = if traffic_pattern == TrafficPattern::AllToOne {
            first_dest
        } else {
            dest_index
        };
        let data_generator: std::option::Option<Box<dyn Iterator<Item = EthernetFrame>>> =
            if active_port_indices.contains(&i) {
                Some(Box::new(FrameGen::new(
                    top,
                    config,
                    i,
                    initial_dest_index,
                    traffic_pattern,
                    packet_payload_bytes,
                    num_send_packets,
                    seed,
                )))
            } else {
                None
            };
        sources.push(
            Source::new_and_register(engine, top, format!("source{i}").as_str(), data_generator)
                .unwrap(),
        );
    }

    let sinks: Sinks = (0..num_ports)
        .map(|i| Sink::new_and_register(engine, top, format!("sink{i}").as_str()).unwrap())
        .collect();

    (sources, sinks)
}
