// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::sync::Arc;
use std::vec;

use steam_components::arbiter::{
    Arbiter, Priority, PriorityRoundRobinPolicy, RoundRobinPolicy, WeightedRoundRobinPolicy,
};
use steam_components::flow_controls::limiter::Limiter;
use steam_components::sinks::Sink;
use steam_components::source::Source;
use steam_components::store::Store;
use steam_components::test_helpers::{
    ArbiterInputData, check_round_robin, priority_policy_test_core,
};
use steam_components::{connect_port, option_box_repeat, rc_limiter};
use steam_engine::port::InPort;
use steam_engine::run_simulation;
use steam_engine::test_helpers::start_test;
use steam_track::entity::Entity;

#[test]
fn source_sink() {
    let mut engine = start_test(file!());
    let spawner = engine.spawner.clone();

    const NUM_PUTS: usize = 25;

    let mut arbiter = Arbiter::new(
        engine.top(),
        "arb",
        spawner,
        3,
        Box::new(RoundRobinPolicy::new()),
    );
    let source_a = Source::new(engine.top(), "source_a", option_box_repeat!(1; NUM_PUTS));
    let source_b = Source::new(engine.top(), "source_b", option_box_repeat!(2; NUM_PUTS));
    let source_c = Source::new(engine.top(), "source_c", option_box_repeat!(3; NUM_PUTS));
    let sink = Sink::new(engine.top(), "sink");

    connect_port!(source_a, tx => arbiter, rx, 0);
    connect_port!(source_b, tx => arbiter, rx, 1);
    connect_port!(source_c, tx => arbiter, rx, 2);
    connect_port!(arbiter, tx => sink, rx);

    let mut sources = vec![source_a, source_b, source_c];

    run_simulation!(engine; sources, [arbiter, sink]);

    let num_sunk = sink.num_sunk();
    assert_eq!(num_sunk, NUM_PUTS * 3);
}

#[test]
fn two_active_inputs() {
    let mut engine = start_test(file!());
    let spawner = engine.spawner.clone();

    let na = 10;
    let nb = 0;
    let nc = 20;

    let mut arbiter = Arbiter::new(
        engine.top(),
        "arb",
        spawner,
        3,
        Box::new(RoundRobinPolicy::new()),
    );
    let source_a = Source::new(engine.top(), "source_a", option_box_repeat!(1; na));
    let source_b = Source::new(engine.top(), "source_b", option_box_repeat!(2; nb));
    let source_c = Source::new(engine.top(), "source_c", option_box_repeat!(3; nc));
    let sink = Sink::new(engine.top(), "sink");

    connect_port!(source_a, tx => arbiter, rx, 0);
    connect_port!(source_b, tx => arbiter, rx, 1);
    connect_port!(source_c, tx => arbiter, rx, 2);
    connect_port!(arbiter, tx => sink, rx);

    let mut sources = vec![source_a, source_b, source_c];

    run_simulation!(engine; sources, [arbiter, sink]);

    let num_sunk = sink.num_sunk();
    assert_eq!(num_sunk, 30);
}

#[test]
fn input_order() {
    let mut engine = start_test(file!());
    let spawner = engine.spawner.clone();

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

    let mut arbiter = Arbiter::new(
        engine.top(),
        "arb",
        spawner.clone(),
        3,
        Box::new(RoundRobinPolicy::new()),
    );
    let source_a = Source::new(
        engine.top(),
        "source_a",
        option_box_repeat!(inputs[0].val; inputs[0].count),
    );
    let source_b = Source::new(
        engine.top(),
        "source_b",
        option_box_repeat!(inputs[1].val; inputs[1].count),
    );
    let source_c = Source::new(
        engine.top(),
        "source_c",
        option_box_repeat!(inputs[2].val; inputs[2].count),
    );
    let total_count = inputs.iter().map(|i| i.count).sum();

    let clock = engine.default_clock();
    let write_limiter = rc_limiter!(clock, 1);
    let store_limiter = Limiter::new(engine.top(), "limit_wr", write_limiter);
    let store = Store::new(engine.top(), "store", spawner, total_count);

    connect_port!(source_a, tx => arbiter, rx, 0);
    connect_port!(source_b, tx => arbiter, rx, 1);
    connect_port!(source_c, tx => arbiter, rx, 2);
    connect_port!(arbiter, tx => store_limiter, rx);
    connect_port!(store_limiter, tx => store, rx);

    let mut sources = vec![source_a, source_b, source_c];

    let port = InPort::new(Arc::new(Entity::new(engine.top(), "port")));
    store.connect_port_tx(port.state());
    engine.spawn(async move {
        let mut store_get = vec![0; total_count];
        for i in &mut store_get {
            *i = port.get().await;
        }

        check_round_robin(&inputs, store_get);
        Ok(())
    });

    run_simulation!(engine; sources, [arbiter, store_limiter, store]);
}

#[test]
#[should_panic]
fn more_inputs() {
    let mut engine = start_test(file!());
    let spawner = engine.spawner.clone();

    let na = 10;
    let nb = 5;
    let nc = 15;

    let mut arbiter = Arbiter::new(
        engine.top(),
        "arb",
        spawner.clone(),
        2,
        Box::new(RoundRobinPolicy::new()),
    );
    let source_a = Source::new(engine.top(), "source_a", option_box_repeat!(1; na));
    let source_b = Source::new(engine.top(), "source_b", option_box_repeat!(2; nb));
    let source_c = Source::new(engine.top(), "source_c", option_box_repeat!(3; nc));
    let store = Store::new(engine.top(), "store", spawner, na + nb + nc);

    connect_port!(source_a, tx => arbiter, rx, 0);
    connect_port!(source_b, tx => arbiter, rx, 1);
    connect_port!(source_c, tx => arbiter, rx, 2);
    connect_port!(arbiter, tx => store, rx);

    let mut sources = vec![source_a, source_b, source_c];

    run_simulation!(engine; sources, [arbiter, store]);
}

#[test]
#[should_panic]
fn no_output() {
    let mut engine = start_test(file!());
    let spawner = engine.spawner.clone();

    let na = 10;
    let nb = 5;
    let nc = 15;

    let arbiter = Arbiter::new(
        engine.top(),
        "arb",
        spawner.clone(),
        3,
        Box::new(RoundRobinPolicy::new()),
    );
    let source_a = Source::new(engine.top(), "source_a", option_box_repeat!(1; na));
    let source_b = Source::new(engine.top(), "source_b", option_box_repeat!(2; nb));
    let source_c = Source::new(engine.top(), "source_c", option_box_repeat!(3; nc));
    let store: Store<i32> = Store::new(engine.top(), "store", spawner, na + nb + nc);

    connect_port!(source_a, tx => arbiter, rx, 0);
    connect_port!(source_b, tx => arbiter, rx, 1);
    connect_port!(source_c, tx => arbiter, rx, 2);

    let mut sources = vec![source_a, source_b, source_c];

    run_simulation!(engine; sources, [arbiter, store]);
}

#[test]
fn weighted_policy() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let spawner = engine.spawner.clone();

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

    let mut arbiter = Arbiter::new(
        engine.top(),
        file!(),
        spawner.clone(),
        num_inputs,
        Box::new(WeightedRoundRobinPolicy::new(weights.clone(), num_inputs)),
    );
    let source_a = Source::new(
        engine.top(),
        "source_a",
        option_box_repeat!(inputs[0].val; inputs[0].count),
    );
    let source_b = Source::new(
        engine.top(),
        "source_b",
        option_box_repeat!(inputs[1].val; inputs[1].count),
    );
    let write_limiter = rc_limiter!(clock, 1);
    let store_limiter = Limiter::new(engine.top(), "limit_wr", write_limiter);
    let store = Store::new(engine.top(), "store", spawner, total_count);

    connect_port!(source_a, tx => arbiter, rx, 0);
    connect_port!(source_b, tx => arbiter, rx, 1);
    connect_port!(arbiter, tx => store_limiter, rx);
    connect_port!(store_limiter, tx => store, rx);

    let port = InPort::new(Arc::new(Entity::new(engine.top(), "port")));
    store.connect_port_tx(port.state());
    engine.spawn(async move {
        let mut store_get = vec![0; total_count];
        for i in &mut store_get {
            *i = port.get().await;
        }

        check_round_robin(&inputs, store_get);
        Ok(())
    });

    let mut sources = vec![source_a, source_b];

    run_simulation!(engine; sources, [arbiter, store_limiter, store]);
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
    engine.run().unwrap();
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
    engine.run().unwrap();
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
    engine.run().unwrap();
}

#[test]
#[should_panic]
fn panic_priority_policy() {
    let engine = start_test(file!());
    let spawner = engine.spawner.clone();

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

    let _arbiter: Arbiter<usize> = Arbiter::new(
        engine.top(),
        "arb",
        spawner,
        num_inputs,
        Box::new(PriorityRoundRobinPolicy::from_priorities(
            priorities.clone(),
            num_inputs + 1,
        )),
    );
}
