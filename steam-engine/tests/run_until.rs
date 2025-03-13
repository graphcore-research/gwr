// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use steam_engine::events::any_of::AnyOf;
use steam_engine::{events::all_of::AllOf, test_helpers::start_test};

mod common;
use common::{create_once_event_at_delay, spawn_activity};

#[test]
fn run_until_once() {
    let mut engine = start_test(file!());

    let once = create_once_event_at_delay(&mut engine, 5, 1);

    spawn_activity(&mut engine);
    engine.run_until(once).unwrap();

    assert_eq!(engine.time_now_ns(), 5.0);
}

#[test]
fn run_until_allof_5_10() {
    let mut engine = start_test(file!());

    let ev_1 = create_once_event_at_delay(&mut engine, 5, 1);
    let ev_2 = create_once_event_at_delay(&mut engine, 10, 2);
    let allof = Box::new(AllOf::new(vec![ev_1, ev_2]));

    spawn_activity(&mut engine);
    engine.run_until(allof).unwrap();

    assert_eq!(engine.time_now_ns(), 10.0);
}

#[test]
fn run_until_allof_10_5() {
    let mut engine = start_test(file!());

    let ev_1 = create_once_event_at_delay(&mut engine, 10, 1);
    let ev_2 = create_once_event_at_delay(&mut engine, 5, 2);
    let allof = Box::new(AllOf::new(vec![ev_1, ev_2]));

    spawn_activity(&mut engine);
    engine.run_until(allof).unwrap();

    assert_eq!(engine.time_now_ns(), 10.0);
}

#[test]
fn run_until_any_of_5_10() {
    let mut engine = start_test(file!());

    let ev_1 = create_once_event_at_delay(&mut engine, 5, 1);
    let ev_2 = create_once_event_at_delay(&mut engine, 10, 2);
    let anyf = Box::new(AnyOf::new(vec![ev_1, ev_2]));

    spawn_activity(&mut engine);
    engine.run_until(anyf).unwrap();

    assert_eq!(engine.time_now_ns(), 5.0);
}

#[test]
fn run_until_any_of_10_5() {
    let mut engine = start_test(file!());

    let ev_1 = create_once_event_at_delay(&mut engine, 10, 1);
    let ev_2 = create_once_event_at_delay(&mut engine, 5, 2);
    let anyof = Box::new(AnyOf::new(vec![ev_1, ev_2]));

    spawn_activity(&mut engine);
    engine.run_until(anyof).unwrap();

    assert_eq!(engine.time_now_ns(), 5.0);
}
