// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cmp::min;
use std::collections::HashMap;
use std::sync::Arc;

use tramway_engine::engine::Engine;
use tramway_engine::port::InPort;
use tramway_track::entity::Entity;

use crate::arbiter::Arbiter;
use crate::arbiter::policy::{Priority, PriorityRoundRobin};
use crate::flow_controls::limiter::Limiter;
use crate::source::Source;
use crate::store::Store;
use crate::{connect_port, option_box_repeat, rc_limiter};

#[derive(Clone)]
pub struct ArbiterInputData {
    pub val: usize,
    pub count: usize,
    pub weight: usize,
    pub priority: Priority,
}

pub fn check_round_robin(inputs: &[ArbiterInputData], data: &[usize]) {
    let total_count: usize = inputs.iter().map(|i| i.count).sum();
    assert_eq!(data.len(), total_count);

    let mut inputs = inputs.to_vec();
    let mut offset = 0;
    loop {
        // Determine the count for each input value in the next window. Note that this
        // copes with inputs producing the same value and inputs not producing
        // their full weight in the window.
        let mut expected_window_counts: HashMap<usize, usize> = HashMap::new();
        let mut window_length = 0;
        let max_priority = inputs
            .iter()
            .map(|i| {
                if i.count > 0 {
                    i.priority
                } else {
                    Priority::default()
                }
            })
            .max()
            .unwrap();
        for input in &mut inputs {
            let value_count = min(input.count, input.weight);
            if input.priority == max_priority && value_count > 0 {
                expected_window_counts
                    .entry(input.val)
                    .and_modify(|e| *e += value_count)
                    .or_insert(value_count);

                window_length += value_count;
                input.count -= value_count;
            }
        }
        if window_length == 0 {
            return;
        }

        let mut window_counts = HashMap::new();
        for value in data.iter().skip(offset).take(window_length) {
            window_counts
                .entry(*value)
                .and_modify(|e| *e += 1)
                .or_insert(1);
        }
        assert_eq!(window_counts, expected_window_counts);

        offset += window_length;
    }
}

pub fn priority_policy_test_core(engine: &mut Engine, inputs: &[ArbiterInputData]) {
    let clock = engine.default_clock();
    let spawner = engine.spawner();
    let num_inputs = inputs.len();
    let total_count = inputs.iter().map(|e| e.count).sum();
    let mut policy = PriorityRoundRobin::new(num_inputs);
    for (i, input) in inputs.iter().enumerate() {
        policy = policy.set_priority(i, input.priority);
    }

    let arbiter = Arbiter::new_and_register(
        engine,
        engine.top(),
        "arb",
        spawner.clone(),
        num_inputs,
        Box::new(policy),
    )
    .unwrap();
    let mut sources = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        sources.push(
            Source::new_and_register(
                engine,
                engine.top(),
                &("source_".to_owned() + &i.to_string()),
                option_box_repeat!(input.val; input.count),
            )
            .unwrap(),
        );
    }

    let write_limiter = rc_limiter!(clock, 1);
    let store_limiter =
        Limiter::new_and_register(engine, engine.top(), "limit_wr", write_limiter).unwrap();
    let store =
        Store::new_and_register(engine, engine.top(), "store", spawner, total_count).unwrap();
    connect_port!(store_limiter, tx => store, rx).unwrap();

    for (i, source) in sources.iter_mut().enumerate() {
        connect_port!(source, tx => arbiter, rx, i).unwrap();
    }
    connect_port!(arbiter, tx => store_limiter, rx).unwrap();

    let port = InPort::new(&Arc::new(Entity::new(engine.top(), "port")), "test_rx");
    store.connect_port_tx(port.state()).unwrap();

    let check_inputs = inputs.to_owned();
    engine.spawn(async move {
        let mut store_get = vec![0; total_count];
        for i in &mut store_get {
            *i = port.get()?.await;
        }

        check_round_robin(&check_inputs, &store_get);
        Ok(())
    });
}
