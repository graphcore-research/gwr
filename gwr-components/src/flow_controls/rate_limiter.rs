// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Provide effective bandwidth limit for a component.
//!
//! A [RateLimiter] is a component that is given a
//! [clock](gwr_engine::time::clock::Clock) and a rate in `bits per tick`.
//! It uses this rate limit to enforce a delay determined by the object that is
//! being rate limited.
//!
//! The [RateLimiter] therefore requires objects to implement the
//! [TotalBytes] trait so that the number of bits of the object can be
//! determined.
//!
//! # Ports
//!
//! This component has the following ports:
//!  - One [input port](gwr_engine::port::InPort): `rx`
//!  - One [output port](gwr_engine::port::OutPort): `tx`
//!
//! # Creating a Rate Limiter
//!
//! [RateLimiter]s should normally be constructed using the
//! [rc_limiter!](crate::rc_limiter) macro. This returns a
//! `Result<Rc<RateLimiter>, SimError>`, rejecting a zero rate. Components
//! normally accept an `Rc<RateLimiter>` as a rate limiter argument.
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
//! # use gwr_components::flow_controls::limiter::Limiter;
//! # use gwr_components::sink::Sink;
//! # use gwr_components::source::Source;
//! # use gwr_components::{connect_port, rc_limiter, option_box_repeat};
//! # use gwr_engine::engine::Engine;
//! # use gwr_engine::run_simulation;
//!
//! // Create the engine.
//! let mut engine = Engine::default();
//!
//! // Create a 1GHz clock.
//! let clock = engine.clock_ghz(1.0);
//!
//! // And build a 16 bits-per-tick rate limiter.
//! let rate_limiter = rc_limiter!(&clock, 16).unwrap();
//!
//! // Build the source (initially with no generator).
//! let source = Source::new_and_register(&engine, engine.top(), "source", None);
//!
//! // Create a packet that uses the source as its trace-control entity.
//! let packet = 0; // TODO implement a packet type to use here
//!
//! // Configure the source to produce ten of these packets.
//! source.set_generator(option_box_repeat!(packet ; 10));
//!
//! // Create the a limiter component to enforce the limit
//! let limiter = Limiter::new_and_register(&engine, &clock, engine.top(), "limit", rate_limiter);
//!
//! // Create the sink to accept these packets.
//! let sink = Sink::new_and_register(&engine, &clock, engine.top(), "sink");
//!
//! // Connect the components.
//! connect_port!(source, tx => limiter, rx)
//!     .expect("should be able to connect `Source` to `Limiter`");
//! connect_port!(limiter, tx => sink, rx)
//!     .expect("should be able to connect `Limiter` to `Sink`");
//!
//! // Run the simulation.
//! run_simulation!(engine);
//!
//! // Ensure the time is as expected.
//! assert_eq!(engine.time_now_ns(), 20.0);
//! ```

use std::marker::PhantomData;

use gwr_engine::time::clock::Clock;
use gwr_engine::traits::TotalBytes;
use gwr_engine::types::{SimError, SimResult};

/// Create a [RateLimiter] wrapped in an [Rc](std::rc::Rc), returning a
/// [`SimError`] if the rate is zero.
///
/// This is the most common form of [RateLimiter] used because all of its
/// methods can be used immutably and therefore it can be shared by any number
/// of components with the same bandwidth limit.
#[macro_export]
macro_rules! rc_limiter {
    ($clock:expr, $bits_per_tick:expr) => {
        $crate::flow_controls::rate_limiter::RateLimiter::new($clock, $bits_per_tick)
            .map(std::rc::Rc::new)
    };
}

#[macro_export]
macro_rules! option_rc_limiter {
    ($clock:expr, $bits_per_tick:expr) => {
        $crate::rc_limiter!($clock, $bits_per_tick).map(Some)
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
    /// Create a rate limiter, rejecting a zero rate.
    pub fn new(clock: &Clock, bits_per_tick: usize) -> Result<Self, SimError> {
        if bits_per_tick == 0 {
            return Err(SimError("rate limiter rate must be positive".into()));
        }
        Ok(Self {
            clock: clock.clone(),
            bits_per_tick,
            phantom: PhantomData,
        })
    }

    /// Wait for the packet's service duration, rejecting invalid arithmetic.
    pub async fn delay(&self, value: &T) -> SimResult {
        self.delay_ticks(self.ticks(value)?).await
    }

    /// Wait for a service duration, checking the deadline at the wait point.
    pub async fn delay_ticks(&self, ticks: u64) -> SimResult {
        self.clock.wait_ticks(ticks)?.await;
        Ok(())
    }

    /// Calculate a packet's service duration without overflowing its bit count.
    pub fn ticks(&self, value: &T) -> Result<u64, SimError> {
        let bits = value
            .total_bytes()
            .checked_mul(8)
            .ok_or_else(|| SimError("rate limiter packet bit count overflows".into()))?;
        self.ticks_from_bits(bits)
    }

    /// Round up to whole ticks, rejecting an unrepresentable duration.
    pub fn ticks_from_bits(&self, bits: usize) -> Result<u64, SimError> {
        u64::try_from(bits.div_ceil(self.bits_per_tick))
            .map_err(|_| SimError("rate limiter duration exceeds u64".into()))
    }
}
