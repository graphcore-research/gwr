// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Library functions to build parts of the sim-fabric application.

use std::rc::Rc;

use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_engine::engine::Engine;
use gwr_models::data_frame::DataFrame;
use gwr_models::fabric::FabricConfig;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_xoshiro::Xoshiro256PlusPlus;

use crate::frame_gen::{FrameGen, TrafficPattern};

// Define some types to aid readability
pub type Sources = Vec<Rc<Source<DataFrame>>>;
pub type Sinks = Vec<Rc<Sink<DataFrame>>>;

#[expect(clippy::too_many_arguments)]
#[must_use]
pub fn build_source_sinks(
    engine: &mut Engine,
    config: &Rc<FabricConfig>,
    traffic_pattern: TrafficPattern,
    overhead_size_bytes: usize,
    payload_size_bytes: usize,
    num_send_frames: usize,
    seed: u64,
    num_active_sources: usize,
) -> (Sources, Sinks, usize) {
    let top = engine.top();

    let num_ports = config.num_ports();
    let mut total_expected_frames = 0;
    let mut sources = Vec::with_capacity(num_ports);

    let mut rng = Xoshiro256PlusPlus::seed_from_u64(seed);

    // Create random set of sources that will be active
    let mut all_port_indices: Vec<usize> = config.port_indices().clone();
    all_port_indices.shuffle(&mut rng);
    let active_port_indices: Vec<usize> = all_port_indices
        .into_iter()
        .take(num_active_sources)
        .collect();

    // Create an random set of initial assigments
    let mut dest_indices: Vec<usize> = config.port_indices().clone();
    dest_indices.shuffle(&mut rng);

    let first_dest = dest_indices[0];

    for (i, dest_index) in dest_indices.drain(..).enumerate() {
        let source_index = config.port_indices()[i];

        let config = config.clone();
        let initial_dest_index = if traffic_pattern == TrafficPattern::AllToOne {
            first_dest
        } else {
            dest_index
        };

        let num_frames_from_source = if active_port_indices.contains(&source_index) {
            match traffic_pattern {
                // These generators won't send anything
                TrafficPattern::AllToOne | TrafficPattern::AllToAllFixed => {
                    if source_index == initial_dest_index {
                        0
                    } else {
                        num_send_frames
                    }
                }
                // All other generators will send the requested number without sending to self.
                _ => num_send_frames,
            }
        } else {
            0
        };

        total_expected_frames += num_frames_from_source;

        let data_generator: std::option::Option<Box<dyn Iterator<Item = DataFrame>>> =
            if active_port_indices.contains(&source_index) {
                Some(Box::new(FrameGen::new(
                    top,
                    config,
                    source_index,
                    initial_dest_index,
                    traffic_pattern,
                    overhead_size_bytes,
                    payload_size_bytes,
                    num_send_frames,
                    seed,
                )))
            } else {
                None
            };
        sources.push(
            Source::new_and_register(
                engine,
                top,
                format!("source{source_index}").as_str(),
                data_generator,
            )
            .unwrap(),
        );
    }

    let sinks: Sinks = config
        .port_indices()
        .iter()
        .map(|i| Sink::new_and_register(engine, top, format!("sink{i}").as_str()).unwrap())
        .collect();

    (sources, sinks, total_expected_frames)
}
