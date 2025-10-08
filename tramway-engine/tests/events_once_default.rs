// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

use tramway_engine::events::once::Once;
use tramway_engine::run_simulation;
use tramway_engine::test_helpers::start_test;
use tramway_engine::traits::Event;

#[test]
fn notify_zero_listeners() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let event = Once::default();

    engine.spawn(async move {
        event.notify()?;
        Ok(())
    });

    run_simulation!(engine);

    assert_eq!(clock.time_now_ns(), 0.0);
}

#[test]
fn notify_one_listener() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let once = Once::default();

    {
        let once = once.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            once.listen().await;
            // Ensure this hasn't completed early
            assert_eq!(clock.time_now_ns(), 10.0);
            Ok(())
        });
    }

    {
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            once.notify()?;
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now_ns(), 10.0);
}

#[test]
fn notify_before_listener() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let once = Once::default();

    {
        let once = once.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            once.listen().await;
            // Ensure this hasn't completed early
            assert_eq!(clock.time_now_ns(), 10.0);
            Ok(())
        });
    }

    engine.spawn(async move {
        once.notify()?;
        Ok(())
    });

    run_simulation!(engine);

    assert_eq!(clock.time_now_ns(), 10.0);
}

#[test]
fn notify_multiple_listeners() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let once = Once::default();

    let count = Rc::new(RefCell::new(0));
    let num_listen = 10;

    for _ in 0..num_listen {
        let once = once.clone();
        let clock = clock.clone();
        let count = count.clone();
        engine.spawn(async move {
            once.listen().await;
            // Ensure this hasn't completed early
            assert_eq!(clock.time_now_ns(), 10.0);
            *count.borrow_mut() += 1;
            Ok(())
        });
    }

    {
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            once.notify()?;
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now_ns(), 10.0);
    assert_eq!(*count.borrow(), num_listen);
}

#[test]
fn notify_twice() {
    let mut engine = start_test(file!());

    let once = Once::default();

    engine.spawn(async move {
        once.notify()?;
        once.notify()?;
        Ok(())
    });

    run_simulation!(engine, "Error: once event already triggered");
}
