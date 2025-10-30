// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::rc::Rc;
use std::vec;

use gwr_components::arbiter::Arbiter;
use gwr_components::arbiter::policy::{
    Priority, PriorityRoundRobin, RoundRobin, WeightedRoundRobin,
};
use gwr_components::flow_controls::limiter::Limiter;
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::store::Store;
use gwr_components::test_helpers::{
    ArbiterInputData, check_round_robin, priority_policy_test_core,
};
use gwr_components::{connect_port, option_box_repeat, rc_limiter};
use gwr_engine::port::InPort;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_track::entity::Entity;

#[test]
fn source_sink() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    const NUM_PUTS: usize = 25;

    let top = engine.top();
    let arbiter =
        Arbiter::new_and_register(&engine, &clock, top, "arb", 3, Box::new(RoundRobin::new()))
            .unwrap();
    let source_a =
        Source::new_and_register(&engine, top, "source_a", option_box_repeat!(1; NUM_PUTS))
            .unwrap();
    let source_b =
        Source::new_and_register(&engine, top, "source_b", option_box_repeat!(2; NUM_PUTS))
            .unwrap();
    let source_c =
        Source::new_and_register(&engine, top, "source_c", option_box_repeat!(3; NUM_PUTS))
            .unwrap();
    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();

    connect_port!(source_a, tx => arbiter, rx, 0).unwrap();
    connect_port!(source_b, tx => arbiter, rx, 1).unwrap();
    connect_port!(source_c, tx => arbiter, rx, 2).unwrap();
    connect_port!(arbiter, tx => sink, rx).unwrap();

    run_simulation!(engine);

    let num_sunk = sink.num_sunk();
    assert_eq!(num_sunk, NUM_PUTS * 3);
}

#[test]
fn two_active_inputs() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let na = 10;
    let nb = 0;
    let nc = 20;

    let top = engine.top();
    let arbiter =
        Arbiter::new_and_register(&engine, &clock, top, "arb", 3, Box::new(RoundRobin::new()))
            .unwrap();
    let source_a =
        Source::new_and_register(&engine, top, "source_a", option_box_repeat!(1; na)).unwrap();
    let source_b =
        Source::new_and_register(&engine, top, "source_b", option_box_repeat!(2; nb)).unwrap();
    let source_c =
        Source::new_and_register(&engine, top, "source_c", option_box_repeat!(3; nc)).unwrap();
    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();

    connect_port!(source_a, tx => arbiter, rx, 0).unwrap();
    connect_port!(source_b, tx => arbiter, rx, 1).unwrap();
    connect_port!(source_c, tx => arbiter, rx, 2).unwrap();
    connect_port!(arbiter, tx => sink, rx).unwrap();

    run_simulation!(engine);

    let num_sunk = sink.num_sunk();
    assert_eq!(num_sunk, 30);
}

#[test]
fn input_order() {
    let mut engine = start_test(file!());

    let inputs = [
        ArbiterInputData {
            val: 1,
            count: 10,
            weight: 1,
            priority: Priority::Low,
        },
        ArbiterInputData {
            val: 2,
            count: 5,
            weight: 1,
            priority: Priority::Low,
        },
        ArbiterInputData {
            val: 3,
            count: 15,
            weight: 1,
            priority: Priority::Low,
        },
    ];

    let clock = engine.default_clock();
    let top = engine.top();
    let arbiter =
        Arbiter::new_and_register(&engine, &clock, top, "arb", 3, Box::new(RoundRobin::new()))
            .unwrap();
    let source_a = Source::new_and_register(
        &engine,
        top,
        "source_a",
        option_box_repeat!(inputs[0].val; inputs[0].count),
    )
    .unwrap();
    let source_b = Source::new_and_register(
        &engine,
        top,
        "source_b",
        option_box_repeat!(inputs[1].val; inputs[1].count),
    )
    .unwrap();
    let source_c = Source::new_and_register(
        &engine,
        top,
        "source_c",
        option_box_repeat!(inputs[2].val; inputs[2].count),
    )
    .unwrap();
    let total_count = inputs.iter().map(|i| i.count).sum();

    let write_limiter = rc_limiter!(&clock, 1);
    let store_limiter =
        Limiter::new_and_register(&engine, &clock, top, "limit_wr", write_limiter).unwrap();
    let store = Store::new_and_register(&engine, &clock, top, "store", total_count).unwrap();

    connect_port!(source_a, tx => arbiter, rx, 0).unwrap();
    connect_port!(source_b, tx => arbiter, rx, 1).unwrap();
    connect_port!(source_c, tx => arbiter, rx, 2).unwrap();
    connect_port!(arbiter, tx => store_limiter, rx).unwrap();
    connect_port!(store_limiter, tx => store, rx).unwrap();

    let port = InPort::new(
        &engine,
        &clock,
        &Rc::new(Entity::new(engine.top(), "port")),
        "test_rx",
    );
    store.connect_port_tx(port.state()).unwrap();
    engine.spawn(async move {
        let mut store_get = vec![0; total_count];
        for i in &mut store_get {
            *i = port.get()?.await;
        }

        check_round_robin(&inputs, &store_get);
        Ok(())
    });

    run_simulation!(engine);
}

#[test]
#[should_panic]
fn more_inputs() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let na = 10;
    let nb = 5;
    let nc = 15;

    let top = engine.top();
    let arbiter =
        Arbiter::new_and_register(&engine, &clock, top, "arb", 2, Box::new(RoundRobin::new()))
            .unwrap();
    let source_a =
        Source::new_and_register(&engine, top, "source_a", option_box_repeat!(1; na)).unwrap();
    let source_b =
        Source::new_and_register(&engine, top, "source_b", option_box_repeat!(2; nb)).unwrap();
    let source_c =
        Source::new_and_register(&engine, top, "source_c", option_box_repeat!(3; nc)).unwrap();
    let store = Store::new_and_register(&engine, &clock, top, "store", na + nb + nc).unwrap();

    connect_port!(source_a, tx => arbiter, rx, 0).unwrap();
    connect_port!(source_b, tx => arbiter, rx, 1).unwrap();
    connect_port!(source_c, tx => arbiter, rx, 2).unwrap();
    connect_port!(arbiter, tx => store, rx).unwrap();

    run_simulation!(engine);
}

#[test]
#[should_panic]
fn no_output() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let na = 10;
    let nb = 5;
    let nc = 15;

    let top = engine.top();
    let arbiter =
        Arbiter::new_and_register(&engine, &clock, top, "arb", 3, Box::new(RoundRobin::new()))
            .unwrap();
    let source_a =
        Source::new_and_register(&engine, top, "source_a", option_box_repeat!(1; na)).unwrap();
    let source_b =
        Source::new_and_register(&engine, top, "source_b", option_box_repeat!(2; nb)).unwrap();
    let source_c =
        Source::new_and_register(&engine, top, "source_c", option_box_repeat!(3; nc)).unwrap();
    let _store: Rc<Store<i32>> =
        Store::new_and_register(&engine, &clock, top, "store", na + nb + nc).unwrap();

    connect_port!(source_a, tx => arbiter, rx, 0).unwrap();
    connect_port!(source_b, tx => arbiter, rx, 1).unwrap();
    connect_port!(source_c, tx => arbiter, rx, 2).unwrap();

    run_simulation!(engine);
}

#[test]
fn weighted_policy() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let inputs = vec![
        ArbiterInputData {
            val: 1,
            count: 30,
            weight: 2,
            priority: Priority::Low,
        },
        ArbiterInputData {
            val: 2,
            count: 20,
            weight: 5,
            priority: Priority::Low,
        },
    ];

    let num_inputs = inputs.len();
    let total_count = inputs.iter().map(|e| e.count).sum();
    let weights: Vec<usize> = inputs.iter().map(|e| e.weight).collect();

    let top = engine.top();
    let arbiter = Arbiter::new_and_register(
        &engine,
        &clock,
        top,
        file!(),
        num_inputs,
        Box::new(WeightedRoundRobin::new(weights.clone(), num_inputs).unwrap()),
    )
    .unwrap();
    let source_a = Source::new_and_register(
        &engine,
        top,
        "source_a",
        option_box_repeat!(inputs[0].val; inputs[0].count),
    )
    .unwrap();
    let source_b = Source::new_and_register(
        &engine,
        top,
        "source_b",
        option_box_repeat!(inputs[1].val; inputs[1].count),
    )
    .unwrap();
    let write_limiter = rc_limiter!(&clock, 1);
    let store_limiter =
        Limiter::new_and_register(&engine, &clock, top, "limit_wr", write_limiter).unwrap();
    let store = Store::new_and_register(&engine, &clock, top, "store", total_count).unwrap();

    connect_port!(source_a, tx => arbiter, rx, 0).unwrap();
    connect_port!(source_b, tx => arbiter, rx, 1).unwrap();
    connect_port!(arbiter, tx => store_limiter, rx).unwrap();
    connect_port!(store_limiter, tx => store, rx).unwrap();

    let port = InPort::new(
        &engine,
        &clock,
        &Rc::new(Entity::new(engine.top(), "port")),
        "test_rx",
    );
    store.connect_port_tx(port.state()).unwrap();
    engine.spawn(async move {
        let mut store_get = vec![0; total_count];
        for i in &mut store_get {
            *i = port.get()?.await;
        }

        check_round_robin(&inputs, &store_get);
        Ok(())
    });

    run_simulation!(engine);
}

#[test]
fn same_priority_policy() {
    let mut engine = start_test(file!());
    let inputs = vec![
        ArbiterInputData {
            val: 1,
            count: 1000,
            weight: 0,
            priority: Priority::Low,
        },
        ArbiterInputData {
            val: 2,
            count: 1500,
            weight: 0,
            priority: Priority::Low,
        },
    ];

    priority_policy_test_core(&mut engine, &inputs);
    run_simulation!(engine);
}

#[test]
fn diff_priority_policy() {
    let mut engine = start_test(file!());
    let inputs = vec![
        ArbiterInputData {
            val: 1,
            count: 1000,
            weight: 0,
            priority: Priority::Low,
        },
        ArbiterInputData {
            val: 2,
            count: 1500,
            weight: 0,
            priority: Priority::Medium,
        },
    ];

    priority_policy_test_core(&mut engine, &inputs);
    run_simulation!(engine);
}

#[test]
fn multiple_inputs_priority_policy() {
    let mut engine = start_test(file!());
    let inputs = vec![
        ArbiterInputData {
            val: 1,
            count: 10,
            weight: 0,
            priority: Priority::Low,
        },
        ArbiterInputData {
            val: 2,
            count: 15,
            weight: 0,
            priority: Priority::Medium,
        },
        ArbiterInputData {
            val: 3,
            count: 10,
            weight: 0,
            priority: Priority::Medium,
        },
        ArbiterInputData {
            val: 4,
            count: 15,
            weight: 0,
            priority: Priority::Low,
        },
    ];

    priority_policy_test_core(&mut engine, &inputs);
    run_simulation!(engine);
}

#[test]
#[should_panic]
fn panic_priority_policy() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let inputs = [
        ArbiterInputData {
            val: 1,
            count: 30,
            weight: 0,
            priority: Priority::Low,
        },
        ArbiterInputData {
            val: 2,
            count: 20,
            weight: 0,
            priority: Priority::Medium,
        },
    ];

    let num_inputs = inputs.len();
    let priorities: Vec<Priority> = inputs.iter().map(|e| e.priority).collect();

    let top = engine.top();
    let _arbiter: Rc<Arbiter<usize>> = Arbiter::new_and_register(
        &engine,
        &clock,
        top,
        "arb",
        num_inputs,
        Box::new(PriorityRoundRobin::from_priorities(priorities.clone(), num_inputs + 1).unwrap()),
    )
    .unwrap();
}
