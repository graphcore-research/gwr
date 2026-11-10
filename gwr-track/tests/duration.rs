// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;

use capnp::serialize_packed;
use gwr_std::time::Duration;
use gwr_track::test_helpers::TestTracker;
use gwr_track::trace_visitor::{TraceVisitor, process_capnp};
use gwr_track::tracker::{CapnProtoTracker, EntityManager, TextTracker};
use gwr_track::{Id, Track, gwr_track_capnp};

#[derive(Clone, Default)]
struct TraceBuffer(Rc<RefCell<Vec<u8>>>);

impl Write for TraceBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct TimeVisitor {
    times: Vec<(Id, Duration)>,
}

impl TraceVisitor for TimeVisitor {
    fn time(&mut self, id: Id, time: Duration) {
        self.times.push((id, time));
    }
}

fn visit_raw_time(seconds: u32, nanosecs: f64) {
    let mut message = capnp::message::Builder::new_default();
    let mut event = message.init_root::<gwr_track_capnp::event::Builder>();
    event.set_id(17);
    let mut time = event.init_time();
    time.set_seconds(seconds);
    time.set_nanosecs(nanosecs);

    let mut bytes = Vec::new();
    serialize_packed::write_message(&mut bytes, &message).unwrap();
    process_capnp(bytes.as_slice(), &mut TimeVisitor::default());
}

#[test]
fn capnp_timestamps_preserve_fractional_and_large_nanoseconds() {
    let buffer = TraceBuffer::default();
    let tracker = CapnProtoTracker::new(
        EntityManager::new(log::Level::Trace),
        Box::new(buffer.clone()),
    );
    let times = [
        Duration::ZERO,
        Duration::new(0, 0.25),
        Duration::new(42, 123_456_789.125),
        Duration::from_nanos(9_007_199_254_740_993),
        Duration::new(u32::MAX, 0.5),
        Duration::new(u32::MAX, 999_999_999.5),
        Duration::MAX,
    ];
    for time in times {
        tracker.time(Id(17), time);
    }
    tracker.shutdown();

    let mut visitor = TimeVisitor::default();
    process_capnp(buffer.0.borrow().as_slice(), &mut visitor);

    assert_eq!(visitor.times, times.map(|time| (Id(17), time)));
}

#[test]
#[should_panic(expected = "trace Duration should be finite, nonnegative and within range")]
fn capnp_rejects_negative_nanoseconds() {
    visit_raw_time(0, -0.25);
}

#[test]
#[should_panic(expected = "trace Duration should be finite, nonnegative and within range")]
fn capnp_rejects_nan_nanoseconds() {
    visit_raw_time(0, f64::NAN);
}

#[test]
#[should_panic(expected = "trace Duration should be finite, nonnegative and within range")]
fn capnp_rejects_infinite_nanoseconds() {
    visit_raw_time(0, f64::INFINITY);
}

#[test]
#[should_panic(expected = "trace Duration should be finite, nonnegative and within range")]
fn capnp_rejects_overflowing_nanoseconds() {
    visit_raw_time(u32::MAX, 1_000_000_000.0);
}

#[test]
fn text_timestamps_preserve_fractional_and_large_nanoseconds() {
    let buffer = TraceBuffer::default();
    let tracker = TextTracker::new(
        EntityManager::new(log::Level::Trace),
        Box::new(buffer.clone()),
    );
    for time in [
        Duration::new(0, 0.25),
        Duration::new(42, 123_456_789.125),
        Duration::from_nanos(9_007_199_254_740_993),
        Duration::new(u32::MAX, 999_999_999.5),
    ] {
        tracker.time(Id(17), time);
    }
    tracker.shutdown();

    assert_eq!(
        String::from_utf8(buffer.0.borrow().clone()).unwrap(),
        concat!(
            "17: set time to 0.25ns\n",
            "17: set time to 42123456789.125ns\n",
            "17: set time to 9007199254740993ns\n",
            "17: set time to 4294967295999999999.5ns\n",
        )
    );
}

#[test]
fn test_tracker_timestamps_preserve_fractional_and_large_nanoseconds() {
    let tracker = TestTracker::new(17, log::Level::Trace);
    tracker.time(Id(17), Duration::new(42, 123_456_789.125));
    tracker.time(Id(17), Duration::new(u32::MAX, 999_999_999.5));

    assert_eq!(
        tracker.events(),
        [
            "17: set time 42123456789.125ns",
            "17: set time 4294967295999999999.5ns",
        ]
    );
}

#[cfg(feature = "perfetto")]
#[test]
fn perfetto_timestamps_explicitly_truncate_fractional_nanoseconds() {
    use gwr_perfetto::protos::Trace;
    use gwr_track::tracker::perfetto::PerfettoTracker;
    use prost::Message;

    let buffer = TraceBuffer::default();
    let tracker = PerfettoTracker::new(
        EntityManager::new(log::Level::Trace),
        Box::new(buffer.clone()),
    );
    for time in [
        Duration::new(0, 0.75),
        Duration::new(42, 123.75),
        Duration::new(9_007_199, 254_740_993.75),
        Duration::MAX,
    ] {
        tracker.time(Id(17), time);
        tracker.value(Id(17), 1.0);
    }

    let trace = Trace::decode(buffer.0.borrow().as_slice()).unwrap();
    assert_eq!(
        trace
            .packet
            .iter()
            .map(|packet| packet.timestamp)
            .collect::<Vec<_>>(),
        [
            Some(0),
            Some(42_000_000_123),
            Some(9_007_199_254_740_993),
            Some(4_294_967_295_999_999_999),
        ]
    );
}
