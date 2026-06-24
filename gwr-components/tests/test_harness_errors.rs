// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use gwr_components::arbiter::Arbiter;
use gwr_components::arbiter::policy::round_robin::RoundRobin;
use gwr_components::build_component_harness;
use gwr_components::delay::Delay;
use gwr_engine::test_helpers::start_test;

fn panic_message(f: impl FnOnce()) -> String {
    let panic = catch_unwind(AssertUnwindSafe(f)).expect_err("expected panic");

    if let Some(message) = panic.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = panic.downcast_ref::<&str>() {
        return (*message).to_owned();
    }
    "<non-string panic>".to_owned()
}

fn assert_panic_contains(f: impl FnOnce(), expected: &str) -> String {
    let message = panic_message(f);
    assert!(
        message.contains(expected),
        "panic message did not contain {expected:?}:\n{message}"
    );
    message
}

fn assert_has_file_line_column(message: &str) {
    let Some(rest) = message.split_once(file!()).map(|(_, rest)| rest) else {
        panic!("panic message did not contain file name:\n{message}");
    };
    let mut parts = rest.trim_start_matches(':').splitn(3, ':');
    let line = parts.next().unwrap_or_default();
    let column = parts
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or_default();

    assert!(line.parse::<u32>().is_ok(), "missing line in:\n{message}");
    assert!(
        column.parse::<u32>().is_ok(),
        "missing column in:\n{message}"
    );
}

mod delay_errors {
    use super::*;

    build_component_harness! {
        harness DelayHarness<T> {
            component: delay: Rc<Delay<T>>,
            rx ports: {
                Rx<T> => rx
            },
            tx ports: {
                Tx<T> => tx
            },
        }
    }

    #[test]
    fn harness_flags_send_step_on_tx_port() {
        let message = assert_panic_contains(
            || {
                let mut engine = start_test(file!());
                let clock = engine.default_clock();
                let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
                let mut harness = DelayHarness::new(engine, delay);

                harness.run_steps([Step::SendRx {
                    location: step_location!(),
                    port: Port::Tx,
                    value: 1,
                }]);
            },
            "Tx: step is for Rx",
        );
        assert_has_file_line_column(&message);
    }

    #[test]
    fn harness_flags_expect_step_on_rx_port() {
        let message = assert_panic_contains(
            || {
                let mut engine = start_test(file!());
                let clock = engine.default_clock();
                let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
                let mut harness = DelayHarness::new(engine, delay);

                harness.run_steps([Step::ExpectTx {
                    location: step_location!(),
                    port: Port::Rx,
                    value: 1,
                }]);
            },
            "Rx: step is for Tx",
        );
        assert_has_file_line_column(&message);
    }

    #[test]
    fn harness_flags_delay_with_ports() {
        let message = assert_panic_contains(
            || {
                let mut engine = start_test(file!());
                let clock = engine.default_clock();
                let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
                let mut harness = DelayHarness::<i32>::new(engine, delay);

                harness.run_steps([Step::Delay {
                    location: step_location!(),
                    ports: vec![Port::Tx],
                    ticks: 1,
                }]);
            },
            "delay does not take ports",
        );
        assert_has_file_line_column(&message);
    }

    #[test]
    fn harness_flags_parallel_step_on_wrong_port() {
        let message = assert_panic_contains(
            || {
                let mut engine = start_test(file!());
                let clock = engine.default_clock();
                let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
                let mut harness = DelayHarness::new(engine, delay);

                harness.run_steps([par!([Step::SendRx {
                    location: step_location!(),
                    port: Port::Tx,
                    value: 1,
                }])]);
            },
            "Tx: step is for Rx",
        );
        assert_has_file_line_column(&message);
    }

    #[test]
    fn harness_flags_parallel_no_traffic_on_rx_port() {
        let message = assert_panic_contains(
            || {
                let mut engine = start_test(file!());
                let clock = engine.default_clock();
                let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
                let mut harness = DelayHarness::<i32>::new(engine, delay);

                harness.run_steps([par!([expect_no_traffic!(&[Port::Rx], 1)])]);
            },
            "Rx: expect no traffic requires tx ports",
        );
        assert_has_file_line_column(&message);
    }

    #[test]
    fn harness_flags_missing_expected_traffic() {
        assert_panic_contains(
            || {
                let mut engine = start_test(file!());
                let clock = engine.default_clock();
                let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
                let mut harness = DelayHarness::new(engine, delay);

                harness.run_steps([expect_tx!(1)]);
            },
            "test harness did not complete",
        );
    }
}

mod arbiter_errors {
    use super::*;

    build_component_harness! {
        harness ArbiterHarness<T> {
            component: arbiter: Rc<Arbiter<T>>,
            tx ports: {
                Tx<T> => tx
            },
            rx port arrays: {
                Rx<T> => rx {
                    count: num_rx
                }
            },
        }
    }

    #[test]
    fn harness_flags_indexed_rx_port_out_of_range() {
        assert_panic_contains(
            || {
                let mut engine = start_test(file!());
                let clock = engine.default_clock();
                let arbiter = Arbiter::new_and_register(
                    &engine,
                    &clock,
                    engine.top(),
                    "arb",
                    1,
                    Box::new(RoundRobin::new()),
                );
                let mut harness = ArbiterHarness::new(engine, arbiter, 1);

                harness.run_steps([send_rx!(1, 99)]);
            },
            "rx driver index 1 out of range or already taken",
        );
    }
}
