// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

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
use gwr_components::store::{ObjectStore, Store};
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
        );
        let mut harness = SourceHarness::new(engine, source);

        harness.run_steps([expect_tx!(1), delay!(3), expect_tx!(2)]);

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
        let sink = Sink::new_and_register(&engine, &clock, engine.top(), "sink");
        let mut harness = SinkHarness::new(engine, sink.clone());

        harness.run_steps([delay!(4), send_rx!(1), send_rx!(2)]);

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
    fn script_runs_async_helpers_with_loops_and_conditionals() {
        let mut harness = create_delay_harness(1);
        harness.run_steps([
            script! {
                ports { input = Rx, output = Tx }
                async {
                    for value in [3, 7] {
                        let received = roundtrip(&mut input, &mut output, value).await?;
                        let next = if received > 5 { 10 } else { 0 };
                        assert_eq!(roundtrip(&mut input, &mut output, next).await?, next);
                    }
                    Ok(())
                }
            },
            send_rx!(20),
            expect_tx!(20),
        ]);
    }

    #[test]
    fn script_runs_concurrent_async_operations() {
        let mut harness = create_delay_harness(1);
        harness.run_steps([script! {
            ports { input = Rx, output = Tx }
            async {
                let (_, received) = futures::join!(input.put(9)?, output.get()?);
                assert_eq!(received, 9);
                input.put(10)?.await;
                assert_eq!(output.get()?.await, 10);
                Ok(())
            }
        }]);
        assert_eq!(harness.clock.tick_now().tick(), 2);
    }

    #[test]
    fn script_selects_values_with_conditional_expressions() {
        for (input_value, expected) in [(7, 10), (3, 0)] {
            let mut harness = create_delay_harness(1);
            harness.run_steps([script! {
                ports { input = Rx, output = Tx }
                async {
                    input.put(input_value)?.await;
                    let received = output.get()?.await;
                    let next = if received > 5 { 10 } else { 0 };
                    input.put(next)?.await;
                    assert_eq!(output.get()?.await, expected);
                    Ok(())
                }
            }]);
        }
    }

    #[test]
    fn cloned_scripts_create_fresh_futures_and_locals() {
        let mut harness = create_delay_harness(1);
        let values = vec![8];
        let script = script! {
            ports { input = Rx, output = Tx }
            async {
                let mut values = values;
                input.put(values.pop().unwrap())?.await;
                let mut received = output.get()?.await;
                received += 1;
                assert_eq!(received, 9);
                Ok(())
            }
        };
        harness.run_steps([script.clone(), script]);
        assert_eq!(harness.clock.tick_now().tick(), 2);
    }

    #[test]
    fn scripts_compose_with_parallel_steps_and_return_ports() {
        let mut harness = create_delay_harness(3);
        let clock = harness.clock.clone();
        harness.run_steps([
            par!([
                send_rx!(7),
                script! {
                    ports { output = Tx }
                    async {
                        let received = output.get()?.await;
                        assert_eq!(received, 7);
                        let next = received + 5;
                        clock.wait_ticks(2)?.await;
                        assert_eq!(next, 12);
                        Ok(())
                    }
                },
            ]),
            script! {
                ports { input = Rx, output = Tx }
                async {
                    input.put(12)?.await;
                    let received = output.get()?.await;
                    input.put(received + 1)?.await;
                    assert_eq!(output.get()?.await, 13);
                    Ok(())
                }
            },
            send_rx!(20),
            expect_tx!(20),
        ]);
        assert_eq!(harness.clock.tick_now().tick(), 14);
    }

    #[test]
    fn script_can_await_without_capturing_ports() {
        let mut harness = create_delay_harness(1);
        let clock = harness.clock.clone();
        harness.run_steps([
            script! {
                ports {}
                async {
                    clock.wait_ticks(2)?.await;
                    Ok(())
                }
            },
            send_rx!(7),
            expect_tx!(7),
        ]);
        assert_eq!(harness.clock.tick_now().tick(), 3);
    }

    #[test]
    fn harness_steps_can_share_a_model_with_an_external_receiver() {
        let mut harness = create_delay_harness(3);
        let mut receiver = harness.take_tx_receiver();
        let received = Rc::new(std::cell::Cell::new(None));
        let observed = received.clone();
        harness.engine.spawn(async move {
            observed.set(Some(receiver.get()?.await));
            Ok(())
        });

        harness.run_steps([send_rx!(7), delay!(4)]);

        assert_eq!(received.get(), Some(7));
    }

    #[test]
    fn harness_steps_can_share_a_model_with_an_external_driver() {
        let mut harness = create_delay_harness(3);
        let mut driver = harness.take_rx_driver();
        harness.engine.spawn(async move {
            driver.put(7)?.await;
            Ok(())
        });

        harness.run_steps([expect_tx!(7)]);
    }

    #[test]
    fn harness_supports_paired_delay_ports() {
        let mut harness = create_delay_harness(3);

        harness.run_steps([
            send_rx!(7),
            expect_no_traffic!(&[Port::Tx], 2),
            expect_tx!(7),
        ]);
    }

    #[test]
    fn script_returns_ports_after_an_early_success() {
        let mut harness = create_delay_harness(1);
        let stop_early = true;
        harness.run_steps([
            script! {
                ports { input = Rx, output = Tx }
                async {
                    if stop_early {
                        return Ok(());
                    }
                    input.put(7)?.await;
                    assert_eq!(output.get()?.await, 7);
                    Ok(())
                }
            },
            send_rx!(9),
            expect_tx!(9),
        ]);
    }

    async fn roundtrip(
        input: &mut gwr_engine::port::OutPort<i32>,
        output: &mut gwr_engine::port::InPort<i32>,
        value: i32,
    ) -> Result<i32, gwr_engine::types::SimError> {
        input.put(value)?.await;
        Ok(output.get()?.await)
    }

    fn create_delay_harness(delay_ticks: usize) -> DelayHarness<i32> {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", delay_ticks);
        DelayHarness::new(engine, delay)
    }
}

mod object_store_harness {
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

    #[test]
    fn harness_supports_object_store_ports() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let store =
            ObjectStore::new_and_register(&engine, &clock, engine.top(), "store", 2).unwrap();
        let mut harness = ObjectStoreHarness::new(engine, store);

        harness.run_steps([send_rx!(10), send_rx!(20), expect_tx!(10), expect_tx!(20)]);
    }

    #[test]
    fn harness_supports_pending_send_steps() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let store =
            ObjectStore::new_and_register(&engine, &clock, engine.top(), "store", 1).unwrap();
        let mut harness = ObjectStoreHarness::new(engine, store);

        harness.run_steps([
            send_rx!(10),
            par!([
                expect_pending_send_rx!(20, 1),
                seq!([delay!(1), expect_tx!(10)]),
            ]),
            expect_tx!(20),
        ]);

        assert_eq!(harness.clock.time_now_ns(), 1.0);
    }

    struct ObjectStoreFillGenerator {
        store: Rc<Store<usize>>,
        next_value: usize,
        next_expected: usize,
        num_to_receive: usize,
        capacity: usize,
    }

    impl Iterator for ObjectStoreFillGenerator {
        type Item = Step<usize>;

        fn next(&mut self) -> Option<Self::Item> {
            if self.next_expected == self.num_to_receive {
                return None;
            }

            if self.store.capacity_used() < self.capacity && self.next_value < self.num_to_receive {
                let value = self.next_value;
                self.next_value += 1;
                return Some(send_rx!(value));
            }

            let value = self.next_expected;
            self.next_expected += 1;
            Some(expect_tx!(value))
        }
    }

    #[test]
    fn harness_supports_stateful_step_generator() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let store =
            ObjectStore::new_and_register(&engine, &clock, engine.top(), "store", 2).unwrap();
        let generator = ObjectStoreFillGenerator {
            store: store.clone(),
            next_value: 0,
            next_expected: 0,
            num_to_receive: 8,
            capacity: 2,
        };
        let mut harness = ObjectStoreHarness::new(engine, store.clone());

        harness.run_step_generator(generator);

        assert_eq!(store.capacity_used(), 0);
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
            Rc::new(RateLimiter::new(&clock, 32).unwrap()),
        );
        let mut harness = LimiterHarness::new(engine, limiter);

        harness.run_steps([par!([send_rx!(4), expect_tx!(4)])]);
    }

    #[test]
    fn harness_supports_nested_seq_par_steps() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let limiter = Limiter::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "limiter",
            Rc::new(RateLimiter::new(&clock, 32).unwrap()),
        );
        let mut harness = LimiterHarness::new(engine, limiter);

        harness.run_steps([par!([
            seq!([send_rx!(4), send_rx!(5)]),
            seq!([expect_tx!(4), expect_tx!(5)]),
        ])]);
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
        );
        let mut harness = ArbiterHarness::new(engine, arbiter, 2);

        harness.run_steps([par!([
            seq!([send_rx!(0, 1), send_rx!(0, 3)]),
            seq!([send_rx!(1, 2), send_rx!(1, 4)]),
            seq!([expect_tx!(1), expect_tx!(2), expect_tx!(3), expect_tx!(4)]),
        ])]);
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
        );
        let mut harness = CreditLimiterHarness::new(engine, limiter);

        harness.run_steps([
            send_rx!(42),
            expect_tx!(42),
            send_rx!(43),
            expect_no_traffic!(&[Port::Tx], 5),
            send_credit_rx!(Credit(1)),
            expect_tx!(43),
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
        let issuer = CreditIssuer::new_and_register(&engine, &clock, engine.top(), "credit_issuer");
        let mut harness = CreditIssuerHarness::new(engine, issuer);

        harness.run_steps([send_rx!(5), expect_credit_tx!(Credit(1)), expect_tx!(5)]);
    }
}

mod indexed_delay_harness {
    use gwr_engine::port::PortStateResult;
    use gwr_engine::traits::SimObject;
    use gwr_engine::types::SimResult;

    use super::*;

    struct IndexedDelays<T: SimObject>(Vec<Rc<Delay<T>>>);

    impl<T: SimObject> IndexedDelays<T> {
        fn port_rx_i(&self, index: usize) -> PortStateResult<T> {
            self.0[index].port_rx()
        }

        fn connect_port_tx_i(&self, index: usize, port: PortStateResult<T>) -> SimResult {
            self.0[index].connect_port_tx(port)
        }
    }

    build_component_harness! {
        harness IndexedDelayHarness<T> {
            component: delays: Rc<IndexedDelays<T>>,
            rx port arrays: { Rx<T> => rx { count: num_rx } },
            tx port arrays: { Tx<T> => tx { count: num_tx } },
        }
    }

    #[test]
    fn script_captures_indexed_port_values() {
        let mut harness = create_indexed_delay_harness();
        let port = 1;
        harness.run_steps([
            par!([
                send_rx!(port, 7),
                script! {
                    ports { output = Tx[port], input = Rx[0] }
                    async {
                        let received = output.get()?.await;
                        assert_eq!(received, 7);
                        input.put(received - 1)?.await;
                        Ok(())
                    }
                },
                expect_tx!(0, 6),
            ]),
            send_rx!(1, 8),
            expect_tx!(1, 8),
        ]);
    }

    #[test]
    fn script_evaluates_indices_once_and_restores_multiple_array_ports() {
        let mut harness = create_indexed_delay_harness();
        let evaluations = Rc::new(std::cell::Cell::new(0));
        let observed = evaluations.clone();
        let script = script! {
            ports {
                first_input = Rx[{
                    let index = observed.get();
                    observed.set(index + 1);
                    index
                }],
                second_input = Rx[1],
                first_output = Tx[0],
                second_output = Tx[1],
            }
            async {
                first_input.put(7)?.await;
                second_input.put(8)?.await;
                assert_eq!(first_output.get()?.await, 7);
                assert_eq!(second_output.get()?.await, 8);
                Ok(())
            }
        };
        assert_eq!(evaluations.get(), 1);
        harness.run_steps([script.clone(), script, send_rx!(0, 9), expect_tx!(0, 9)]);
        assert_eq!(evaluations.get(), 1);
    }

    fn create_indexed_delay_harness() -> IndexedDelayHarness<i32> {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delays = Rc::new(IndexedDelays(
            (0..2)
                .map(|index| {
                    Delay::new_and_register(
                        &engine,
                        &clock,
                        engine.top(),
                        &format!("delay_{index}"),
                        1,
                    )
                })
                .collect(),
        ));
        IndexedDelayHarness::new(engine, delays, 2, 2)
    }
}
