// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::{FutureExt, select};
use gwr_engine::events::repeated::Repeated;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::Event;

pub mod common;
use common::{counting_waker, wake_count};

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
            assert_eq!(clock.time_now(), Duration::from_nanos(10));
            Ok(())
        });
    }

    {
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            repeated.notify();
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now(), Duration::from_nanos(10));
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
            assert_eq!(clock.time_now(), Duration::from_nanos(10));
            Ok(())
        });
    }

    {
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            repeated.notify_result(result);
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now(), Duration::from_nanos(10));
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
        repeated.notify();
        Ok(())
    });

    run_simulation!(engine);

    assert_eq!(clock.time_now(), Duration::from_nanos(10));
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
            assert_eq!(clock.time_now(), Duration::from_nanos(10));
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

            repeated.notify();
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now(), Duration::from_nanos(10));
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
            assert_eq!(clock.time_now(), Duration::from_nanos(10));
            assert_eq!(res, usize::default());

            let res = repeated.listen().await;

            // Ensure this hasn't completed early
            assert_eq!(clock.time_now(), Duration::from_nanos(20));
            assert_eq!(res, usize::default());
            Ok(())
        });
    }

    {
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            repeated.notify();
            clock.wait_ticks(10).await;
            repeated.notify();
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now(), Duration::from_nanos(20));
}

#[test]
fn notify_listen_loop() {
    const COUNTER: usize = 10;

    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let repeated = Repeated::new(usize::default());

    {
        let repeated = repeated.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            for i in 0..COUNTER {
                let res = repeated.listen().await;
                assert_eq!(res, i + 1);
                assert_eq!(
                    clock.time_now(),
                    Duration::from_nanos((10 * (i + 1)) as u64)
                );
            }
            Ok(())
        });
    }

    {
        let clock = clock.clone();
        engine.spawn(async move {
            for i in 0..COUNTER {
                clock.wait_ticks(10).await;
                repeated.notify_result(i + 1);
            }
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(
        clock.time_now(),
        Duration::from_nanos((10 * COUNTER) as u64)
    );
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
            assert_eq!(clock.time_now(), Duration::from_nanos(10));

            let _ = repeated.listen().await;
            panic!("I should not be awoken twice");
        });
    }

    {
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            repeated.notify();
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now(), Duration::from_nanos(10));
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
            assert_eq!(clock.time_now(), Duration::from_nanos(10));
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
            repeated.notify();
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now(), Duration::from_nanos(11));
}

#[test]
fn repolling_listener_replaces_registered_waker() {
    let repeated = Repeated::new(123);
    let mut listener = repeated.listen();

    let (first_wakes, first_waker) = counting_waker();
    let (second_wakes, second_waker) = counting_waker();

    let mut cx = Context::from_waker(&first_waker);
    assert_eq!(listener.as_mut().poll(&mut cx), Poll::Pending);

    let mut cx = Context::from_waker(&second_waker);
    assert_eq!(listener.as_mut().poll(&mut cx), Poll::Pending);

    repeated.notify_result(456);

    assert_eq!(wake_count(&first_wakes), 0);
    assert_eq!(wake_count(&second_wakes), 1);
    assert_eq!(listener.as_mut().poll(&mut cx), Poll::Ready(456));
}

#[test]
fn cancelled_listener_is_removed() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let repeated = Repeated::new(usize::default());

    {
        let repeated = repeated.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            {
                let mut listener = repeated.listen().fuse();
                let mut timeout = clock.wait_ticks(5).fuse();

                select! {
                    _ = listener => panic!("listener should have been cancelled"),
                    () = timeout => {}
                }
            }

            clock.wait_ticks(5).await;
            Ok(())
        });
    }

    {
        let repeated = repeated.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(10).await;
            repeated.notify();
            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(clock.time_now(), Duration::from_nanos(10));
}
