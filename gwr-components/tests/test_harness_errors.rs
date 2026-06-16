// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_components::arbiter::Arbiter;
use gwr_components::arbiter::policy::round_robin::RoundRobin;
use gwr_components::build_component_harness;
use gwr_components::delay::Delay;
use gwr_engine::test_helpers::start_test;

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
    #[should_panic(expected = "step 0 Tx: step is for Rx")]
    fn harness_flags_send_step_on_tx_port() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
        let mut harness = DelayHarness::new(engine, delay);

        harness.run_steps([Step::SendRx {
            port: Port::Tx,
            value: 1,
        }]);
    }

    #[test]
    #[should_panic(expected = "step 0 Rx: step is for Tx")]
    fn harness_flags_expect_step_on_rx_port() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
        let mut harness = DelayHarness::new(engine, delay);

        harness.run_steps([Step::ExpectTx {
            port: Port::Rx,
            value: 1,
        }]);
    }

    #[test]
    #[should_panic(expected = "step 0: delay does not take ports")]
    fn harness_flags_delay_with_ports() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
        let mut harness = DelayHarness::<i32>::new(engine, delay);

        harness.run_steps([Step::Delay {
            ports: vec![Port::Tx],
            ticks: 1,
        }]);
    }

    #[test]
    #[should_panic(expected = "step 0: parallel step 0 step 0 Tx: step is for Rx")]
    fn harness_flags_parallel_step_on_wrong_port() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
        let mut harness = DelayHarness::new(engine, delay);

        harness.run_steps([step_par([Step::SendRx {
            port: Port::Tx,
            value: 1,
        }])]);
    }

    #[test]
    #[should_panic(
        expected = "step 0: parallel step 0 step 0 Rx: expect no traffic requires tx ports"
    )]
    fn harness_flags_parallel_no_traffic_on_rx_port() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
        let mut harness = DelayHarness::<i32>::new(engine, delay);

        harness.run_steps([step_par([step_expect_no_traffic(&[Port::Rx], 1)])]);
    }

    #[test]
    #[should_panic(expected = "test harness did not complete")]
    fn harness_flags_missing_expected_traffic() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
        let mut harness = DelayHarness::new(engine, delay);

        harness.run_steps([step_expect_tx(1)]);
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
    #[should_panic(expected = "rx driver index 1 out of range or already taken")]
    fn harness_flags_indexed_rx_port_out_of_range() {
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

        harness.run_steps([step_send_rx(1, 99)]);
    }
}
