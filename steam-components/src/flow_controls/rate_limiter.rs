// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Provide effective bandwidth limit for a component.
//!
//! A [RateLimiter] is a component that is given a
//! [clock](steam_engine::time::clock::Clock) and a rate in `bits per tick`. It
//! uses this rate limit to enforce a delay determined by the object that is
//! being rate limited.
//!
//! The [RateLimiter] therefore requires objects to implement the
//! [TotalBytes] trait so that the number of bits of the object can be
//! determined.
//!
//! # Creating a Rate Limiter
//!
//! [RateLimiter]s should normally be constructed using the
//! [rc_limiter!](crate::rc_limiter) macro. This returns an `Rc<RateLimiter>`
//! because that is what components normally accept as a rate limiter argument.
//! They are `Rc`ed because they are used immutably and as a result the same
//! rate limiter can be shared by all components that have the same bandwidth.
//!
//! # Examples:
//!
//! Here is a basic example of a rate limiter being used by the
//! [Limiter](crate::flow_controls::limiter) component which is connected
//! between a source and sink.
//!
//! A [source](crate::source::Source) is used to produce 4-byte packets.
//! The rate limiter is configured to run on a 1GHz clock at a rate of 16 bits
//! per tick.
//!
//! As a result, the total time for the simulation should be `20.0ns` because
//! each of the 10 packets should take 2 clock ticks to pass through the
//! [Limiter](crate::flow_controls::limiter) and be consumed by the
//! [Sink](crate::sink::Sink).
//!
//! ```rust
//! # use steam_components::flow_controls::limiter::Limiter;
//! # use steam_components::sink::Sink;
//! # use steam_components::source::Source;
//! # use steam_components::{connect_port, rc_limiter, option_box_repeat};
//! # use steam_engine::engine::Engine;
//! # use steam_engine::run_simulation;
//!
//! // Create the engine.
//! let mut engine = Engine::default();
//!
//! // Create a 1GHz clock.
//! let clock = engine.clock_ghz(1.0);
//!
//! // And build a 16 bits-per-tick rate limiter.
//! let rate_limiter = rc_limiter!(clock, 16);
//!
//! // Build the source (initially with no generator).
//! let source = Source::new(engine.top(), "source", None);
//!
//! // Create a packet that uses the source as its trace-control entity.
//! let packet = 0; // TODO implement a packet type to use here
//!
//! // Configure the source to produce ten of these packets.
//! source.set_generator(option_box_repeat!(packet ; 10));
//!
//! // Create the a limiter component to enforce the limit
//! let limiter = Limiter::new(engine.top(), "limit", rate_limiter);
//!
//! // Create the sink to accept these packets.
//! let sink = Sink::new(engine.top(), "sink");
//!
//! // Connect the components.
//! connect_port!(source, tx => limiter, rx);
//! connect_port!(limiter, tx => sink, rx);
//!
//! // Run the simulation.
//! run_simulation!(engine ; [source, limiter,  sink]);
//!
//! // Ensure the time is as expected.
//! assert_eq!(engine.time_now_ns(), 20.0);
//! ```

use std::marker::PhantomData;

use steam_engine::time::clock::Clock;
use steam_engine::traits::TotalBytes;

/// Create a [RateLimiter] wrapped in an [Rc](std::rc::Rc).
///
/// This is the most common form of [RateLimiter] used because all of its
/// methods can be used immutably and therefore it can be shared by any number
/// of components with the same bandwidth limit.
#[macro_export]
macro_rules! rc_limiter {
    ($clock:expr, $bits_per_tick:expr) => {
        std::rc::Rc::new($crate::flow_controls::rate_limiter::RateLimiter::new(
            $clock,
            $bits_per_tick,
        ))
    };
}

#[macro_export]
macro_rules! option_rc_limiter {
    ($clock:expr, $bits_per_tick:expr) => {
        Some(std::rc::Rc::new(
            $crate::flow_controls::rate_limiter::RateLimiter::new($clock, $bits_per_tick),
        ))
    };
}

#[derive(Clone)]
pub struct RateLimiter<T>
where
    T: TotalBytes,
{
    /// Clock rate limiter is attached to.
    clock: Clock,

    /// Bits per tick that can pass through this interface.
    bits_per_tick: usize,

    phantom: PhantomData<T>,
}

impl<T> RateLimiter<T>
where
    T: TotalBytes,
{
    pub fn new(clock: Clock, bits_per_tick: usize) -> Self {
        Self {
            clock,
            bits_per_tick,
            phantom: PhantomData,
        }
    }

    pub async fn delay(&self, value: &T) {
        let delay_ticks = self.ticks(value);
        self.clock.wait_ticks(delay_ticks as u64).await;
    }

    pub async fn delay_ticks(&self, ticks: usize) {
        self.clock.wait_ticks(ticks as u64).await;
    }

    pub fn ticks(&self, value: &T) -> usize {
        let payload_bytes = value.total_bytes();
        let payload_bits = payload_bytes * 8;
        self.ticks_from_bits(payload_bits)
    }

    pub fn ticks_from_bits(&self, bits: usize) -> usize {
        bits.div_ceil(self.bits_per_tick)
    }
}
