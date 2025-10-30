// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Library functions to build parts of the sim-ring application.

use std::rc::Rc;

use gwr_components::arbiter::policy::WeightedRoundRobin;
use gwr_components::flow_controls::limiter::Limiter;
use gwr_components::rc_limiter;
use gwr_components::router::Route;
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_engine::engine::Engine;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::Routable;
use gwr_engine::types::SimError;
use gwr_models::ethernet_frame::{EthernetFrame, u64_to_mac};
use gwr_models::fc_pipeline::{FcPipeline, FcPipelineConfig};
use gwr_models::ring_node::{IO_INDEX, RING_INDEX, RingConfig, RingNode};

use crate::frame_gen::FrameGen;

// Define some types to aid readability
pub type Limiters = Vec<Rc<Limiter<EthernetFrame>>>;
pub type Nodes = Vec<Rc<RingNode<EthernetFrame>>>;
pub type Pipes = Vec<Rc<FcPipeline<EthernetFrame>>>;
pub type Sources = Vec<Rc<Source<EthernetFrame>>>;
pub type Sinks = Vec<Rc<Sink<EthernetFrame>>>;

pub struct Config {
    pub ring_size: usize,
    pub ring_priority: usize,
    pub rx_buffer_frames: usize,
    pub tx_buffer_frames: usize,
    pub frame_payload_bytes: usize,
    pub num_send_frames: usize,
}

struct RoutingAlgorithm(usize);

impl<T> Route<T> for RoutingAlgorithm
where
    T: Routable,
{
    fn route(&self, obj: &T) -> Result<usize, SimError> {
        // If the dest matches then exit via port 1, otherwise use port 0 as that is the
        // ring
        let dest = obj.destination() as usize;
        Ok(if self.0 == dest { IO_INDEX } else { RING_INDEX })
    }
}

pub fn build_ring_nodes(engine: &mut Engine, clock: &Clock, config: &Config) -> Nodes {
    let limiter_128_gbps = rc_limiter!(clock, 128);
    let ring_config = RingConfig::new(
        config.rx_buffer_frames,
        config.tx_buffer_frames,
        limiter_128_gbps.clone(),
    );
    let top = engine.top();
    let ring_nodes: Nodes = (0..config.ring_size)
        .map(|i| {
            let weights = vec![config.ring_priority, 1];
            RingNode::new_and_register(
                engine,
                clock,
                top,
                &format!("node_{i}"),
                &ring_config,
                Box::new(RoutingAlgorithm(i)),
                Box::new(WeightedRoundRobin::new(weights, 2).unwrap()),
            )
            .unwrap()
        })
        .collect();
    ring_nodes
}

pub fn build_source_sinks(engine: &mut Engine, clock: &Clock, config: &Config) -> (Sources, Sinks) {
    let mut sources = Vec::with_capacity(config.ring_size);
    let top = engine.top();

    for i in 0..config.ring_size {
        let neighbour_left = if i > 0 { i - 1 } else { config.ring_size - 1 };

        sources.push(
            Source::new_and_register(
                engine,
                top,
                &format!("source_{i}"),
                Some(Box::new(FrameGen::new(
                    top,
                    u64_to_mac(neighbour_left as u64),
                    config.frame_payload_bytes,
                    config.num_send_frames,
                ))),
            )
            .unwrap(),
        );
    }

    let sinks: Sinks = (0..config.ring_size)
        .map(|i| Sink::new_and_register(engine, clock, top, &format!("sink_{i}")).unwrap())
        .collect();

    (sources, sinks)
}

pub fn build_pipes(engine: &mut Engine, clock: &Clock, config: &Config) -> (Pipes, Pipes) {
    let mut ingress_pipes = Vec::with_capacity(config.ring_size);
    let mut ring_pipes = Vec::with_capacity(config.ring_size);

    let top = engine.top();

    let pipe_config = FcPipelineConfig::new(500, 500, 500);
    for i in 0..config.ring_size {
        ingress_pipes.push(
            FcPipeline::new_and_register(
                engine,
                clock,
                top,
                &format!("ingress_pipe_{i}"),
                &pipe_config,
            )
            .unwrap(),
        );
        ring_pipes.push(
            FcPipeline::new_and_register(
                engine,
                clock,
                top,
                &format!("ring_pipe_{i}"),
                &pipe_config,
            )
            .unwrap(),
        );
    }
    (ingress_pipes, ring_pipes)
}

pub fn build_limiters(
    engine: &mut Engine,
    clock: &Clock,
    config: &Config,
    gbps: usize,
) -> (Limiters, Limiters, Limiters) {
    let limiter_gbps = rc_limiter!(clock, gbps);
    let top = engine.top();
    let source_limiters: Limiters = (0..config.ring_size)
        .map(|i| {
            Limiter::new_and_register(
                engine,
                clock,
                top,
                &format!("src_limit_{i}"),
                limiter_gbps.clone(),
            )
            .unwrap()
        })
        .collect();

    let ring_limiters: Limiters = (0..config.ring_size)
        .map(|i| {
            Limiter::new_and_register(
                engine,
                clock,
                top,
                &format!("ring_limit_{i}"),
                limiter_gbps.clone(),
            )
            .unwrap()
        })
        .collect();

    let sink_limiters: Limiters = (0..config.ring_size)
        .map(|i| {
            Limiter::new_and_register(
                engine,
                clock,
                top,
                &format!("sink_limit_{i}"),
                limiter_gbps.clone(),
            )
            .unwrap()
        })
        .collect();
    (source_limiters, ring_limiters, sink_limiters)
}
