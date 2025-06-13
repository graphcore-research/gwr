// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use steam_engine::events::any_of::AnyOf;
use steam_engine::test_helpers::start_test;
use steam_engine::traits::Event;

mod common;
use common::{create_once_event_at_delay, spawn_activity};

#[derive(Clone, Copy)]
enum EventResults {
    Result1,
    Result2,
}

#[test]
fn anyof_once_and_anyof_once() {
    let mut engine = start_test(file!());

    let ev_1 = create_once_event_at_delay(&mut engine, 20, 1);
    let anyof_1 = Box::new(AnyOf::new(vec![ev_1]));

    let ev_2 = create_once_event_at_delay(&mut engine, 10, 2);
    let anyof_2 = Box::new(AnyOf::new(vec![anyof_1, ev_2]));

    spawn_activity(&mut engine);
    engine.run_until(anyof_2).unwrap();

    assert_eq!(engine.time_now_ns(), 10.0);
}

#[test]
fn multiple_listen() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let ev_1 = create_once_event_at_delay(&mut engine, 10, 1);
    let ev_2 = create_once_event_at_delay(&mut engine, 20, 2);
    let ev_3 = create_once_event_at_delay(&mut engine, 30, 3);

    let anyof = AnyOf::new(vec![ev_1, ev_2, ev_3]);

    {
        let anyof = anyof.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            let res = anyof.listen().await;
            assert_eq!(res, 1);
            assert_eq!(clock.time_now_ns(), 10.0);
            Ok(())
        });
    }

    engine.spawn(async move {
        clock.wait_ticks(1).await;
        let res = anyof.listen().await;
        assert_eq!(res, 1);
        assert_eq!(clock.time_now_ns(), 10.0);
        Ok(())
    });

    engine.run().unwrap();

    // The simulation doesn't complete until all events have fired
    assert_eq!(engine.time_now_ns(), 30.0);
}

#[test]
fn using_enum() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let ev_1 = create_once_event_at_delay(&mut engine, 10, EventResults::Result1);
    let ev_2 = create_once_event_at_delay(&mut engine, 5, EventResults::Result2);
    let anyof = AnyOf::new(vec![ev_1, ev_2]);

    engine.spawn(async move {
        match anyof.listen().await {
            EventResults::Result1 => panic!("Wrong event fired"),
            EventResults::Result2 => println!("Correct event fired"),
        }
        assert_eq!(clock.time_now_ns(), 5.0);
        Ok(())
    });

    engine.run().unwrap();
    assert_eq!(engine.time_now_ns(), 10.0);
}
