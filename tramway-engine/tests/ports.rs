// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use futures::select;
use tramway_engine::port::{InPort, OutPort};
use tramway_engine::run_simulation;
use tramway_engine::test_helpers::start_test;

#[test]
fn put_get_synced() {
    let mut engine = start_test(file!());

    let mut tx_port = OutPort::new(engine.top(), "tx");
    let rx_port = InPort::new(engine.top(), "rx");

    tx_port.connect(rx_port.state()).unwrap();

    {
        let clock = engine.default_clock();
        engine.spawn(async move {
            // Do put before any gets happen
            tx_port.put(1)?.await;

            // The `put()` should not have completed until the matching `get()` happens
            assert!(clock.time_now_ns() == 1.0);

            tx_port.put(2)?.await;
            Ok(())
        });
    }

    {
        let clock = engine.default_clock();
        engine.spawn(async move {
            clock.wait_ticks(1).await;
            let i = rx_port.get()?.await;
            assert_eq!(i, 1);
            let i = rx_port.get()?.await;
            assert_eq!(i, 2);

            // Time should not change for any other reason than the `wait_ticks()`
            assert!(clock.time_now_ns() == 1.0);

            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(engine.time_now_ns(), 1.0);
}

#[test]
fn select_on_ports() {
    let mut engine = start_test(file!());

    let mut tx_port1 = OutPort::new(engine.top(), "tx");
    let rx_port1 = InPort::new(engine.top(), "rx");
    tx_port1.connect(rx_port1.state()).unwrap();

    let mut tx_port2 = OutPort::new(engine.top(), "tx");
    let rx_port2 = InPort::new(engine.top(), "rx");
    tx_port2.connect(rx_port2.state()).unwrap();

    {
        let clock = engine.default_clock();
        engine.spawn(async move {
            clock.wait_ticks(1).await;
            tx_port1.put(1)?.await;

            // Time will depend on order of select
            let ns = clock.time_now_ns();
            assert!(ns == 1.0 || ns == 3.0);

            tx_port1.put(3)?.await;
            assert_eq!(clock.time_now_ns(), 5.0);
            Ok(())
        });
    }
    {
        let clock = engine.default_clock();
        engine.spawn(async move {
            clock.wait_ticks(1).await;
            tx_port2.put(2)?.await;

            // Time will depend on order of select
            let ns = clock.time_now_ns();
            if ns == 1.0 {
                clock.wait_ticks(10).await;
            } else {
                assert_eq!(ns, 3.0);
                clock.wait_ticks(8).await;
            }
            tx_port2.put(4)?.await;
            Ok(())
        });
    }

    {
        let clock = engine.default_clock();
        engine.spawn(async move {
            let mut rx1 = rx_port1.get()?;
            let mut rx2 = rx_port2.get()?;

            let mut received = Vec::new();
            loop {
                let i = select! {
                    a = rx1 => {
                        assert!((a & 0x1) == 1);
                        rx1 = rx_port1.get()?;
                        a
                    }
                    b = rx2 => {
                        assert!((b & 0x1) == 0);
                        rx2 = rx_port2.get()?;
                        b
                    }
                };
                received.push(i);

                if received.len() == 4 {
                    break;
                }

                clock.wait_ticks(2).await;
            }
            // The second value from rx2 should be last
            assert_eq!(received[3], 4);

            received.sort();
            // All values should be received, but order of first two is not guaranteed
            assert_eq!(received, [1, 2, 3, 4]);
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(engine.time_now_ns(), 11.0);
}
