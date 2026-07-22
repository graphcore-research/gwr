// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use gwr_components::store::ObjectStore;
use gwr_engine::test_helpers::start_test;

mod object_store_harness {
    use std::rc::Rc;

    use gwr_components::build_component_harness;
    use gwr_components::store::Store;

    use super::*;

    build_component_harness! {
        harness ObjectStoreHarness<T> {
            component: store: Rc<Store<T>>,
            rx ports: {
                Rx<T> => rx
            },
            tx ports: {
                Tx<T> => tx
            },
        }
    }

    /// Basic end-to-end test of the ObjectStore's input and output ports.
    ///
    /// Verifies:
    ///  * all values make it through the store
    ///  * the store is empty at the end (capacity_used == 0)
    #[test]
    fn object_store_basic_flow() {
        const NUM_PUTS: usize = 50;
        const CAPACITY: usize = 8;
        const VALUE: i32 = 1;

        let mut engine = start_test(file!());
        let clock = engine.default_clock();

        let top = engine.top();

        let store = ObjectStore::new_and_register(&engine, &clock, top, "store", CAPACITY).unwrap();
        let mut harness = ObjectStoreHarness::new(engine, store.clone());

        let mut sends = Vec::new();
        let mut expects = Vec::new();

        for _ in 0..NUM_PUTS {
            sends.push(send_rx!(VALUE));
            expects.push(expect_tx!(VALUE));
        }

        harness.run_steps([par!([seq!(sends), seq!(expects)])]);

        assert_eq!(store.capacity_used(), 0);
    }

    /// When `set_error_on_overflow` is enabled, overflowing the object store
    /// should cause the simulation to fail with an overflow error.
    ///
    /// The harness driver keeps pushing data into the store while the
    /// store's `tx` receiver is left unread, causing the store to overflow.
    /// Expect the overflow path in `State::push_value` to be hit.
    ///
    /// NOTE: we only match a substring of the message to avoid depending
    /// on the exact formatting of the entity name.
    #[test]
    #[should_panic(expected = "Overflow in")]
    fn object_store_overflow_panics_when_error_on_overflow_set() {
        const CAPACITY: usize = 2;
        const NUM_PUTS: usize = 10;

        let mut engine = start_test(file!());
        let clock = engine.default_clock();

        let top = engine.top();

        let store = ObjectStore::new_and_register(&engine, &clock, top, "store_overflow", CAPACITY)
            .unwrap();

        // Switch to "error on overflow" mode so `run_rx` no longer blocks once full
        // and instead allows `State::push_value` to return a SimError.
        store.set_error_on_overflow();

        let mut harness = ObjectStoreHarness::new(engine, store.clone());

        let mut sends = Vec::new();
        for _ in 0..NUM_PUTS {
            sends.push(send_rx!(1));
        }

        harness.run_steps(sends);
    }
}
/// Creating an object store with zero capacity should fail with a SimError.
///
/// This directly exercises the constructor error path.
#[test]
fn object_store_zero_capacity_fails() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let top = engine.top();

    let result = ObjectStore::<i32>::new_and_register(&engine, &clock, top, "store_zero", 0);

    assert!(
        result.is_err(),
        "Expected zero-capacity ObjectStore construction to return an error"
    );

    let err = result.err().unwrap();
    let msg = err.to_string(); // Display impl prefixes with "Error: "
    assert!(
        msg.contains("Unsupported Store with capacity of 0"),
        "Unexpected error message: {msg}"
    );
}
