// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::test_helpers::start_test;
use gwr_models::ethernet_frame::{EthernetFrame, FRAME_OVERHEAD_BYTES};
use gwr_models::ethernet_link::{self, EthernetLink};

mod ethernet_link_harness {
    use gwr_components::build_component_harness;

    use super::*;

    build_component_harness! {
        harness EthernetLinkHarness<T> {
            component: link: Rc<EthernetLink<T>>,
            rx ports: {
                RxA<T> => rx_a,
                RxB<T> => rx_b,
            },
            tx ports: {
                TxA<T> => tx_a,
                TxB<T> => tx_b,
            },
        }
    }
    #[test]
    fn change_delay() {
        let delay_ticks = 100;
        let value = 42;

        let mut engine = start_test(file!());
        let clock = engine.clock_ghz(1.0);
        let top = engine.top();

        let link = EthernetLink::<i32>::new_and_register(&engine, &clock, top, "link").unwrap();
        link.set_delay(delay_ticks).unwrap();

        let mut harness = EthernetLinkHarness::new(engine, link);

        harness.run_steps([send_rx_a!(value), expect_tx_a!(value)]);

        assert_eq!(harness.clock.time_now_ns(), delay_ticks as f64);
    }

    #[test]
    fn latency() {
        let value = 42;

        let mut engine = start_test(file!());
        let clock = engine.clock_ghz(1.0);
        let top = engine.top();

        let link = EthernetLink::<i32>::new_and_register(&engine, &clock, top, "link").unwrap();

        let mut harness = EthernetLinkHarness::new(engine, link);

        harness.run_steps([par!([
            send_rx_a!(value),
            seq!([
                expect_no_traffic!(
                    &[Port::TxA, Port::TxB],
                    (ethernet_link::DELAY_TICKS - 1) as u64
                ),
                expect_tx_a!(value),
            ]),
        ])]);

        assert_eq!(
            harness.clock.time_now_ns(),
            ethernet_link::DELAY_TICKS as f64
        );
    }

    #[test]
    fn source_sink() {
        let num_puts_a = 100;
        let num_puts_b = 50;
        let value_a = 42;
        let value_b = 43;

        let mut engine = start_test(file!());
        let clock = engine.clock_ghz(1.0);
        let top = engine.top();

        let link = EthernetLink::<i32>::new_and_register(&engine, &clock, top, "link").unwrap();

        let mut harness = EthernetLinkHarness::new(engine, link);

        harness.run_steps([
            par!([
                seq!(vec![send_rx_a!(value_a); num_puts_a]),
                seq!(vec![send_rx_b!(value_b); num_puts_b]),
                seq!(vec![expect_tx_a!(value_a); num_puts_a]),
                seq!(vec![expect_tx_b!(value_b); num_puts_b]),
            ]),
            expect_no_traffic!(&[Port::TxA, Port::TxB], 1),
        ]);
    }

    #[test]
    fn change_delay_after_simulation_started_errors() {
        let delay_ticks = 100;

        let mut engine = start_test(file!());
        let clock = engine.clock_ghz(1.0);
        let top = engine.top();

        let link = EthernetLink::<i32>::new_and_register(&engine, &clock, top, "link").unwrap();
        let mut harness = EthernetLinkHarness::new(engine, link);

        // Starting the simulation causes the component to take ownership of its
        // ports.
        harness.run_steps([delay!(1)]);

        let error = harness.link.set_delay(delay_ticks).unwrap_err();

        assert_eq!(
            format!("{error}"),
            "top::link::a: can't change the delay after the simulation has started"
        );
    }

    #[test]
    fn throughput() {
        let num_puts = 1000;
        let payload_bytes = 128;

        let mut engine = start_test(file!());
        let clock = engine.clock_ghz(1.0);
        let top = engine.top();

        let frame = EthernetFrame::new(top, payload_bytes);
        let link =
            EthernetLink::<EthernetFrame>::new_and_register(&engine, &clock, top, "link").unwrap();
        let mut harness = EthernetLinkHarness::new(engine, link);

        let frame_bits = (payload_bytes + FRAME_OVERHEAD_BYTES) * 8;
        let frame_ticks = frame_bits.div_ceil(ethernet_link::BITS_PER_TICK);
        let expected_ticks = ethernet_link::DELAY_TICKS + frame_ticks * (num_puts - 1);

        harness.run_steps([par!([
            seq!(vec![send_rx_a!(frame.clone()); num_puts]),
            seq!([
                seq!(vec![expect_tx_a!(frame); num_puts]),
                expect_no_traffic!(&[Port::TxA, Port::TxB], 1),
            ]),
        ])]);

        assert_eq!(harness.clock.tick_now().tick(), expected_ticks as u64 + 1);
    }
}
