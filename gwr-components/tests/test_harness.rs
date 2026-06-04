// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::rc::Rc;

use gwr_components::arbiter::Arbiter;
use gwr_components::arbiter::policy::round_robin::RoundRobin;
use gwr_components::build_component_harness;
use gwr_components::delay::Delay;
use gwr_components::flow_controls::credit_issuer::CreditIssuer;
use gwr_components::flow_controls::credit_limiter::CreditLimiter;
use gwr_components::flow_controls::limiter::Limiter;
use gwr_components::flow_controls::rate_limiter::RateLimiter;
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::store::Store;
use gwr_components::types::Credit;
use gwr_engine::test_helpers::start_test;

mod source_harness {
    use super::*;

    build_component_harness! {
        harness SourceHarness<T> {
            component: source: Rc<Source<T>>,
            tx ports: {
                Tx<T> => tx
            },
        }
    }

    #[test]
    fn harness_supports_tx_only_source() {
        let engine = start_test(file!());
        let source = Source::new_and_register(
            &engine,
            engine.top(),
            "source",
            Some(Box::new([1, 2].into_iter())),
        )
        .unwrap();
        let mut harness = SourceHarness::new(engine, source);

        harness.run_steps(&[step_expect_tx(1), step_delay(3), step_expect_tx(2)]);

        assert_eq!(harness.clock.tick_now().tick(), 3);
    }
}

mod sink_harness {
    use super::*;

    build_component_harness! {
        harness SinkHarness<T> {
            component: sink: Rc<Sink<T>>,
            rx ports: {
                Rx<T> => rx
            },
        }
    }

    #[test]
    fn harness_supports_rx_only_sink() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let sink = Sink::new_and_register(&engine, &clock, engine.top(), "sink").unwrap();
        let mut harness = SinkHarness::new(engine, sink.clone());

        harness.run_steps(&[step_delay(4), step_send_rx(1), step_send_rx(2)]);

        assert_eq!(sink.num_sunk(), 2);
        assert_eq!(harness.clock.tick_now().tick(), 4);
    }
}

mod delay_harness {
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
    fn harness_supports_paired_delay_ports() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 3).unwrap();
        let mut harness = DelayHarness::new(engine, delay);

        harness.run_steps(&[
            step_send_rx(7),
            step_expect_no_traffic(&[Port::Tx], 2),
            step_expect_tx(7),
        ]);
    }
}

mod store_harness {
    use super::*;

    build_component_harness! {
        harness StoreHarness<T> {
            component: store: Rc<Store<T>>,
            rx ports: {
                Rx<T> => rx
            },
            tx ports: {
                Tx<T> => tx
            },
        }
    }

    #[test]
    fn harness_supports_store_ports() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let store = Store::new_and_register(&engine, &clock, engine.top(), "store", 2).unwrap();
        let mut harness = StoreHarness::new(engine, store);

        harness.run_steps(&[
            step_send_rx(10),
            step_send_rx(20),
            step_expect_tx(10),
            step_expect_tx(20),
        ]);
    }

    struct StoreFillGenerator {
        store: Rc<Store<usize>>,
        next_value: usize,
        next_expected: usize,
        num_to_receive: usize,
        capacity: usize,
    }

    impl Iterator for StoreFillGenerator {
        type Item = Step<usize>;

        fn next(&mut self) -> Option<Self::Item> {
            if self.next_expected == self.num_to_receive {
                return None;
            }

            if self.store.fill_level() < self.capacity && self.next_value < self.num_to_receive {
                let value = self.next_value;
                self.next_value += 1;
                return Some(step_send_rx(value));
            }

            let value = self.next_expected;
            self.next_expected += 1;
            Some(step_expect_tx(value))
        }
    }

    #[test]
    fn harness_supports_stateful_step_generator() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let store = Store::new_and_register(&engine, &clock, engine.top(), "store", 2).unwrap();
        let generator = StoreFillGenerator {
            store: store.clone(),
            next_value: 0,
            next_expected: 0,
            num_to_receive: 8,
            capacity: 2,
        };
        let mut harness = StoreHarness::new(engine, store.clone());

        harness.run_step_generator(generator);

        assert_eq!(store.fill_level(), 0);
    }
}

mod limiter_harness {
    use super::*;

    build_component_harness! {
        harness LimiterHarness<T> {
            component: limiter: Rc<Limiter<T>>,
            rx ports: {
                Rx<T> => rx
            },
            tx ports: {
                Tx<T> => tx
            },
        }
    }

    #[test]
    fn harness_supports_limiter_ports() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let limiter = Limiter::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "limiter",
            Rc::new(RateLimiter::new(&clock, 32)),
        )
        .unwrap();
        let mut harness = LimiterHarness::new(engine, limiter);

        harness.run_steps(&[step_parallel(HashMap::from([
            (Port::Rx, vec![action_send_rx(4)]),
            (Port::Tx, vec![action_expect_tx(4)]),
        ]))]);
    }
}

mod arbiter_harness {
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
    fn harness_supports_indexed_arbiter_rx_ports() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let arbiter = Arbiter::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "arb",
            2,
            Box::new(RoundRobin::new()),
        )
        .unwrap();
        let mut harness = ArbiterHarness::new(engine, arbiter, 2);

        harness.run_steps(&[step_parallel(HashMap::from([
            (Port::Rx(0), vec![action_send_rx(1), action_send_rx(3)]),
            (Port::Rx(1), vec![action_send_rx(2), action_send_rx(4)]),
            (
                Port::Tx,
                vec![
                    action_expect_tx(1),
                    action_expect_tx(2),
                    action_expect_tx(3),
                    action_expect_tx(4),
                ],
            ),
        ]))]);
    }
}

mod credit_limiter_harness {
    use super::*;

    build_component_harness! {
        harness CreditLimiterHarness<T> {
            component: limiter: Rc<CreditLimiter<T>>,
            rx ports: {
                Rx<T> => rx,
                CreditRx<Credit> => credit_rx
            },
            tx ports: {
                Tx<T> => tx
            },
        }
    }

    #[test]
    fn harness_supports_credit_limiter_data_ports() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let limiter = CreditLimiter::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "credit_limiter",
            None,
            1,
        )
        .unwrap();
        let mut harness = CreditLimiterHarness::new(engine, limiter);

        harness.run_steps(&[
            step_send_rx(42),
            step_expect_tx(42),
            step_send_rx(43),
            step_expect_no_traffic(&[Port::Tx], 5),
            step_send_credit_rx(Credit(1)),
            step_expect_tx(43),
        ]);
    }
}

mod credit_issuer_harness {
    use super::*;

    build_component_harness! {
        harness CreditIssuerHarness<T> {
            component: issuer: Rc<CreditIssuer<T>>,
            rx ports: {
                Rx<T> => rx
            },
            tx ports: {
                Tx<T> => tx,
                CreditTx<Credit> => credit_tx
            },
        }
    }

    #[test]
    fn harness_supports_credit_issuer_ports() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let issuer =
            CreditIssuer::new_and_register(&engine, &clock, engine.top(), "credit_issuer").unwrap();
        let mut harness = CreditIssuerHarness::new(engine, issuer);

        harness.run_steps(&[
            step_send_rx(5),
            step_expect_credit_tx(Credit(1)),
            step_expect_tx(5),
        ]);
    }
}
