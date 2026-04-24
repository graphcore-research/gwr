// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;
use std::task::{Context, Poll};

use gwr_engine::events::once::Once;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::Event;

pub mod common;
use common::{counting_waker, wake_count};

#[test]
fn notify_zero_listeners() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let event = Once::with_value(1);

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

    let once = Once::with_value(123);

    {
        let once = once.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            let res = once.listen().await;
            assert_eq!(res, 123);

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

    let once = Once::with_value(1234);

    {
        let once = once.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            let res = once.listen().await;
            assert_eq!(res, 1234);

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

    let once = Once::with_value("ok");

    let count = Rc::new(RefCell::new(0));
    let num_listen = 10;

    for _ in 0..num_listen {
        let once = once.clone();
        let clock = clock.clone();
        let count = count.clone();
        engine.spawn(async move {
            let res = once.listen().await;
            assert_eq!(res, "ok");

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

            once.notify()?;
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now_ns(), 10.0);
    assert_eq!(*count.borrow(), num_listen);
}

#[test]
fn repolling_listener_replaces_registered_waker() {
    let once = Once::new(123);
    let mut listener = once.listen();

    let (first_wakes, first_waker) = counting_waker();
    let (second_wakes, second_waker) = counting_waker();

    let mut cx = Context::from_waker(&first_waker);
    assert_eq!(listener.as_mut().poll(&mut cx), Poll::Pending);

    let mut cx = Context::from_waker(&second_waker);
    assert_eq!(listener.as_mut().poll(&mut cx), Poll::Pending);

    once.notify().unwrap();

    assert_eq!(wake_count(&first_wakes), 0);
    assert_eq!(wake_count(&second_wakes), 1);
    assert_eq!(listener.as_mut().poll(&mut cx), Poll::Ready(123));
}

#[test]
fn notify_twice() {
    let mut engine = start_test(file!());

    let once = Once::new("don't care");

    engine.spawn(async move {
        once.notify()?;
        once.notify()?;
        Ok(())
    });

    run_simulation!(engine, "once event already triggered");
}
