// Copyright (c) 2025 Graphcore Ltd. All rights reserved.
//
//! Library functions to build parts of the sim-fabric application.

use std::rc::Rc;

use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_xoshiro::Xoshiro256PlusPlus;
use tramway_components::sink::Sink;
use tramway_components::source::Source;
use tramway_engine::engine::Engine;
use tramway_models::ethernet_frame::EthernetFrame;
use tramway_models::fabric::FabricConfig;

use crate::packet_gen::{PacketGen, TrafficPattern};

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
) -> (Sources, Sinks) {
    let top = engine.top();

    let num_ports = config.num_ports();
    let mut sources = Vec::with_capacity(num_ports);

    let mut rng = Xoshiro256PlusPlus::seed_from_u64(seed);

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
        sources.push(
            Source::new_and_register(
                engine,
                top,
                format!("source{i}").as_str(),
                Some(Box::new(PacketGen::new(
                    top,
                    config,
                    i,
                    initial_dest_index,
                    traffic_pattern,
                    packet_payload_bytes,
                    num_send_packets,
                    seed,
                ))),
            )
            .unwrap(),
        );
    }

    let sinks: Sinks = (0..num_ports)
        .map(|i| Sink::new_and_register(engine, top, format!("sink{i}").as_str()).unwrap())
        .collect();

    (sources, sinks)
}
