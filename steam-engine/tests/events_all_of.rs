// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use steam_engine::events::all_of::AllOf;
use steam_engine::test_helpers::start_test;
use steam_engine::traits::Event;

mod common;
use common::{create_once_event_at_delay, spawn_activity};

#[test]
fn allof_once_and_allof_once() {
    let mut engine = start_test(file!());

    let ev_1 = create_once_event_at_delay(&mut engine, 20, 1);
    let allof_1 = Box::new(AllOf::new(vec![ev_1]));

    let ev_2 = create_once_event_at_delay(&mut engine, 10, 2);
    let allof_2 = Box::new(AllOf::new(vec![allof_1, ev_2]));

    spawn_activity(&mut engine);
    engine.run_until(allof_2).unwrap();

    assert_eq!(engine.time_now_ns(), 20.0);
}

#[test]
fn multiple_listen() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let ev_1 = create_once_event_at_delay(&mut engine, 10, 1);
    let ev_2 = create_once_event_at_delay(&mut engine, 20, 2);
    let ev_3 = create_once_event_at_delay(&mut engine, 30, 3);

    let allof = AllOf::new(vec![ev_1, ev_2, ev_3]);

    {
        let allof = allof.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            allof.listen().await;
            assert_eq!(clock.time_now_ns(), 30.0);
            Ok(())
        });
    }

    engine.spawn(async move {
        clock.wait_ticks(1).await;
        allof.listen().await;
        assert_eq!(clock.time_now_ns(), 30.0);
        Ok(())
    });

    engine.run().unwrap();

    assert_eq!(engine.time_now_ns(), 30.0);
}

#[test]
fn default_value() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let ev_1 = create_once_event_at_delay(&mut engine, 10, 1);
    let allof = AllOf::new(vec![ev_1]);

    engine.spawn(async move {
        let res = allof.listen().await;
        assert_eq!(res, i32::default());

        assert_eq!(clock.time_now_ns(), 10.0);
        Ok(())
    });

    engine.run().unwrap();
    assert_eq!(engine.time_now_ns(), 10.0);
}
