// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fmt;
use std::rc::Rc;

use gwr_components::build_component_harness;
use gwr_components::store::{ByteStore, Store};
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::{SimObject, TotalBytes};
use gwr_track::Id;
use gwr_track::id::Unique;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ByteObject {
    id: Id,
    bytes: usize,
}

impl ByteObject {
    fn new(id: u64, bytes: usize) -> Self {
        Self { id: Id(id), bytes }
    }
}

impl TotalBytes for ByteObject {
    fn total_bytes(&self) -> usize {
        self.bytes
    }
}

impl Unique for ByteObject {
    fn id(&self) -> Id {
        self.id
    }
}

impl fmt::Display for ByteObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "byte object {} ({} bytes)", self.id.0, self.bytes)
    }
}

impl SimObject for ByteObject {}

build_component_harness! {
    harness ByteStoreHarness<T> {
        component: store: Rc<Store<T>>,
        rx ports: {
            Rx<T> => rx
        },
        tx ports: {
            Tx<T> => tx
        },
    }
}

fn byte_store_harness(
    capacity_bytes: usize,
) -> (Rc<Store<ByteObject>>, ByteStoreHarness<ByteObject>) {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let store =
        ByteStore::new_and_register(&engine, &clock, engine.top(), "byte_store", capacity_bytes)
            .unwrap();
    let harness = ByteStoreHarness::new(engine, store.clone());

    (store, harness)
}

#[test]
fn byte_store_basic_flow() {
    let (store, mut harness) = byte_store_harness(8);

    harness.run_steps([
        send_rx!(ByteObject::new(1, 3)),
        send_rx!(ByteObject::new(2, 5)),
        expect_tx!(ByteObject::new(1, 3)),
        expect_tx!(ByteObject::new(2, 5)),
        send_rx!(ByteObject::new(3, 8)),
        expect_tx!(ByteObject::new(3, 8)),
    ]);

    assert_eq!(store.capacity_used(), 0);
}

#[test]
fn byte_store_push_waits_until_enough_bytes_are_available() {
    let (store, mut harness) = byte_store_harness(8);

    harness.run_steps([
        send_rx!(ByteObject::new(1, 4)),
        send_rx!(ByteObject::new(2, 4)),
        par!([
            expect_pending_send_rx!(ByteObject::new(3, 4), 1),
            seq!([delay!(1), expect_tx!(ByteObject::new(1, 4))]),
        ]),
        expect_tx!(ByteObject::new(2, 4)),
        expect_tx!(ByteObject::new(3, 4)),
    ]);

    assert_eq!(store.capacity_used(), 0);
    assert_eq!(harness.clock.time_now_ns(), 1.0);
}

#[test]
fn byte_store_zero_capacity_fails() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let top = engine.top();

    let result = ByteStore::<ByteObject>::new_and_register(&engine, &clock, top, "store_zero", 0);

    assert!(result.is_err());
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("Unsupported Store with capacity of 0")
    );
}

#[test]
#[should_panic(expected = "Overflow in")]
fn byte_store_overflow_panics_when_value_fits_but_store_is_full() {
    let (store, mut harness) = byte_store_harness(8);
    store.set_error_on_overflow();

    harness.run_steps([
        send_rx!(ByteObject::new(1, 8)),
        send_rx!(ByteObject::new(2, 4)),
    ]);
}

#[test]
#[should_panic(expected = "Cannot store 9 bytes")]
fn byte_store_oversized_object_panics() {
    let (_store, mut harness) = byte_store_harness(8);

    harness.run_steps([send_rx!(ByteObject::new(1, 9))]);
}

#[test]
#[should_panic(expected = "Cannot store 9 bytes")]
fn byte_store_oversized_object_panics_when_error_on_overflow_set() {
    let (store, mut harness) = byte_store_harness(8);
    store.set_error_on_overflow();

    harness.run_steps([send_rx!(ByteObject::new(1, 9))]);
}

#[test]
#[should_panic(expected = "Cannot store 9 flits")]
fn byte_store_capacity_unit_can_be_overridden() {
    let (store, mut harness) = byte_store_harness(8);
    store.set_capacity_unit("flits");

    harness.run_steps([send_rx!(ByteObject::new(1, 9))]);
}
