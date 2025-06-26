// Copyright (c) 2025 Graphcore Ltd. All rights reserved.
//
//! Library functions to build parts of the SimRing application.

use steam_components::arbiter::WeightedRoundRobinPolicy;
use steam_components::flow_controls::limiter::Limiter;
use steam_components::rc_limiter;
use steam_components::router::Route;
use steam_components::sink::Sink;
use steam_components::source::Source;
use steam_engine::engine::Engine;
use steam_engine::traits::Routable;
use steam_models::ethernet_frame::{EthernetFrame, u64_to_mac};
use steam_models::fc_pipeline::FcPipeline;
use steam_models::ring_node::{IO_INDEX, RING_INDEX, RingConfig, RingNode};

use crate::packet_gen::PacketGen;

// Define some types to aid readability
pub type Limiters = Vec<Limiter<EthernetFrame>>;
pub type Nodes = Vec<RingNode<EthernetFrame>>;
pub type Pipes = Vec<FcPipeline<EthernetFrame>>;
pub type Sources = Vec<Source<EthernetFrame>>;
pub type Sinks = Vec<Sink<EthernetFrame>>;

pub struct Config {
    pub ring_size: usize,
    pub ring_priority: usize,
    pub rx_buffer_frames: usize,
    pub tx_buffer_frames: usize,
    pub packet_payload_bytes: usize,
    pub num_send_packets: usize,
}

struct Router(usize);

impl<T> Route<T> for Router
where
    T: Routable,
{
    fn route(&self, obj: &T) -> usize {
        // If the dest matches then exit via port 1, otherwise use port 0 as that is the
        // ring
        let dest = obj.dest() as usize;
        if self.0 == dest { IO_INDEX } else { RING_INDEX }
    }
}

pub fn build_ring_nodes(engine: &mut Engine, config: &Config) -> Nodes {
    let spawner = engine.spawner().clone();
    let clock = engine.default_clock().clone();
    let limiter_128_gbps = rc_limiter!(clock.clone(), 128);
    let ring_config = RingConfig::new(config.rx_buffer_frames, config.tx_buffer_frames);
    let top = engine.top();
    let ring_nodes: Nodes = (0..config.ring_size)
        .map(|i| {
            let weights = vec![config.ring_priority, 1];
            RingNode::new(
                top,
                format!("node{i}").as_str(),
                spawner.clone(),
                &ring_config,
                limiter_128_gbps.clone(),
                Box::new(Router(i)),
                Box::new(WeightedRoundRobinPolicy::new(weights, 2)),
            )
        })
        .collect();
    ring_nodes
}

pub fn build_source_sinks(engine: &mut Engine, config: &Config) -> (Sources, Sinks) {
    let mut sources = Vec::with_capacity(config.ring_size);
    let top = engine.top();

    for i in 0..config.ring_size {
        let neighbour_left = if i > 0 { i - 1 } else { config.ring_size - 1 };

        sources.push(Source::new(
            top,
            format!("source{i}").as_str(),
            Some(Box::new(PacketGen::new(
                top,
                u64_to_mac(neighbour_left as u64),
                config.packet_payload_bytes,
                config.num_send_packets,
            ))),
        ));
    }

    let sinks: Sinks = (0..config.ring_size)
        .map(|i| Sink::new(top, format!("sink{i}").as_str()))
        .collect();

    (sources, sinks)
}

pub fn build_pipes(engine: &mut Engine, config: &Config) -> (Pipes, Pipes) {
    let mut ingress_pipes = Vec::with_capacity(config.ring_size);
    let mut ring_pipes = Vec::with_capacity(config.ring_size);

    let clock = engine.default_clock();
    let top = engine.top();
    let spawner = engine.spawner();
    for i in 0..config.ring_size {
        ingress_pipes.push(FcPipeline::new(
            top,
            format!("ingress_pipe{i}").as_str(),
            clock.clone(),
            spawner.clone(),
            500,
            500,
            500,
        ));
        ring_pipes.push(FcPipeline::new(
            top,
            format!("ring_pipe{i}").as_str(),
            clock.clone(),
            spawner.clone(),
            500,
            500,
            500,
        ));
    }
    (ingress_pipes, ring_pipes)
}

pub fn build_limiters(
    engine: &mut Engine,
    config: &Config,
    gbps: usize,
) -> (Limiters, Limiters, Limiters) {
    let clock = engine.default_clock();
    let limiter_gbps = rc_limiter!(clock, gbps);
    let top = engine.top();
    let source_limiters: Limiters = (0..config.ring_size)
        .map(|i| Limiter::new(top, format!("src_limit{i}").as_str(), limiter_gbps.clone()))
        .collect();

    let ring_limiters: Limiters = (0..config.ring_size)
        .map(|i| Limiter::new(top, format!("ring_limit{i}").as_str(), limiter_gbps.clone()))
        .collect();

    let sink_limiters: Limiters = (0..config.ring_size)
        .map(|i| Limiter::new(top, format!("sink_limit{i}").as_str(), limiter_gbps.clone()))
        .collect();
    (source_limiters, ring_limiters, sink_limiters)
}
