// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

use steam_engine::events::repeated::Repeated;
use steam_engine::run_simulation;
use steam_engine::test_helpers::start_test;
use steam_engine::traits::Event;

#[test]
fn notify_one_listener() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let repeated = Repeated::new(usize::default());

    {
        let repeated = repeated.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            let res = repeated.listen().await;
            assert_eq!(res, usize::default());

            // Ensure this hasn't completed early
            assert_eq!(clock.time_now_ns(), 10.0);
            Ok(())
        });
    }

    {
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            repeated.notify()?;
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now_ns(), 10.0);
}

#[test]
fn notify_one_listener_result() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let result = 42;
    let repeated = Repeated::new(42);

    {
        let repeated = repeated.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            let res = repeated.listen().await;
            assert_eq!(res, result);

            // Ensure this hasn't completed early
            assert_eq!(clock.time_now_ns(), 10.0);
            Ok(())
        });
    }

    {
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            repeated.notify_result(result)?;
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now_ns(), 10.0);
}

#[test]
fn notify_before_listen() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let repeated = Repeated::new(usize::default());

    {
        let repeated = repeated.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            let _ = repeated.listen().await;
            panic!("I should not be awoken");
        });
    }

    engine.spawn(async move {
        repeated.notify()?;
        Ok(())
    });

    run_simulation!(engine);

    assert_eq!(clock.time_now_ns(), 10.0);
}

#[test]
fn notify_multiple_listeners() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let repeated = Repeated::new("");

    let count = Rc::new(RefCell::new(0));
    let num_listen = 10;

    for _ in 0..num_listen {
        let repeated = repeated.clone();
        let clock = clock.clone();
        let count = count.clone();
        engine.spawn(async move {
            let res = repeated.listen().await;
            assert_eq!(res, "");

            // Ensure this hasn't completed early
            assert_eq!(clock.time_now_ns(), 10.0);
            *count.borrow_mut() += 1;
            Ok(())
        });
    }

    {
        let clock = clock.clone();
        let count = count.clone();
        engine.spawn(async move {
            // Should be 0 to start with
            assert_eq!(*count.borrow(), 0);
            clock.wait_ticks(10).await;

            // Should still be 0 after delay
            assert_eq!(*count.borrow(), 0);

            repeated.notify()?;
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now_ns(), 10.0);
    assert_eq!(*count.borrow(), num_listen);
}

#[test]
fn notify_listen_twice() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let repeated = Repeated::new(usize::default());

    {
        let repeated = repeated.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            let res = repeated.listen().await;

            // Ensure this hasn't completed early
            assert_eq!(clock.time_now_ns(), 10.0);
            assert_eq!(res, usize::default());

            let res = repeated.listen().await;

            // Ensure this hasn't completed early
            assert_eq!(clock.time_now_ns(), 20.0);
            assert_eq!(res, usize::default());
            Ok(())
        });
    }

    {
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            repeated.notify()?;
            clock.wait_ticks(10).await;
            repeated.notify()?;
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now_ns(), 20.0);
}

#[test]
fn notify_listen_loop() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let repeated = Repeated::new(usize::default());
    const COUNTER: usize = 10;

    {
        let repeated = repeated.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            for i in 0..COUNTER {
                let res = repeated.listen().await;
                assert_eq!(res, i + 1);
                assert_eq!(clock.time_now_ns(), 10.0 * (i + 1) as f64);
            }
            Ok(())
        });
    }

    {
        let clock = clock.clone();
        engine.spawn(async move {
            for i in 0..COUNTER {
                clock.wait_ticks(10).await;
                repeated.notify_result(i + 1)?;
            }
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now_ns(), 10.0 * COUNTER as f64);
}

#[test]
fn notify_once_listen_twice() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let repeated = Repeated::new(usize::default());

    {
        let repeated = repeated.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            let res = repeated.listen().await;
            assert_eq!(res, usize::default());

            // Ensure this hasn't completed early
            assert_eq!(clock.time_now_ns(), 10.0);

            let _ = repeated.listen().await;
            panic!("I should not be awoken twice");
        });
    }

    {
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            repeated.notify()?;
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now_ns(), 10.0);
}

#[test]
fn notify_repeated_two_listeners() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let repeated = Repeated::new(usize::default());

    // Listener 1
    {
        let repeated = repeated.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            let res = repeated.listen().await;
            assert_eq!(res, usize::default());
            assert_eq!(clock.time_now_ns(), 10.0);
            Ok(())
        });
    }

    // Listener 2 - starts later
    {
        let repeated = repeated.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(11).await;
            let _ = repeated.listen().await;
            panic!("I should not be awoken");
        });
    }

    {
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            repeated.notify()?;
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now_ns(), 11.0);
}
