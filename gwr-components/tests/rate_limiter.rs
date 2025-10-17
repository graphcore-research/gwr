// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::fmt::Display;

use gwr_components::flow_controls::rate_limiter::RateLimiter;
use gwr_engine::engine::Engine;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::{Routable, SimObject, TotalBytes};
use gwr_engine::types::AccessType;
use gwr_track::id::{Id, Unique};

#[derive(Clone, Debug)]
struct RateLimiterTest {
    total_bytes: usize,
}

impl TotalBytes for RateLimiterTest {
    fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

impl Routable for RateLimiterTest {
    fn destination(&self) -> u64 {
        0
    }
    fn access_type(&self) -> AccessType {
        AccessType::Read
    }
}

impl Display for RateLimiterTest {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "limiter")
    }
}

impl Unique for RateLimiterTest {
    fn id(&self) -> Id {
        Id(0)
    }
}

impl SimObject for RateLimiterTest {}

impl RateLimiterTest {
    fn new(total_bytes: usize) -> Self {
        Self { total_bytes }
    }
}

fn test_rate_limiter(
    engine: &mut Engine,
    clock_mhz: f64,
    bits_per_tick: usize,
    object: RateLimiterTest,
) {
    let clock = engine.clock_mhz(clock_mhz);
    let rate_limiter = RateLimiter::new(clock, bits_per_tick);

    engine.spawn(async move {
        rate_limiter.delay(&object).await;
        Ok(())
    });

    engine.run().unwrap();
}

#[test]
fn one_ghz_1_byte_8_bits() {
    let mut engine = start_test(file!());
    test_rate_limiter(&mut engine, 1000.0, 8, RateLimiterTest::new(1));
    assert_eq!(engine.time_now_ns(), 1.0);
}

#[test]
fn one_ghz_1_byte_1_bit() {
    let mut engine = start_test(file!());
    test_rate_limiter(&mut engine, 1000.0, 1, RateLimiterTest::new(1));
    assert_eq!(engine.time_now_ns(), 8.0);
}

#[test]
fn one_mhz_1_byte_8_bits() {
    let mut engine = start_test(file!());
    test_rate_limiter(&mut engine, 1.0, 8, RateLimiterTest::new(1));
    assert_eq!(engine.time_now_ns(), 1000.0);
}

#[test]
fn one_ghz_1_byte_7_bits() {
    let mut engine = start_test(file!());
    test_rate_limiter(&mut engine, 1000.0, 7, RateLimiterTest::new(1));
    assert_eq!(engine.time_now_ns(), 2.0);
}

#[test]
fn one_ghz_2_byte_8_bits() {
    let mut engine = start_test(file!());
    test_rate_limiter(&mut engine, 1000.0, 7, RateLimiterTest::new(1));
    assert_eq!(engine.time_now_ns(), 2.0);
}
