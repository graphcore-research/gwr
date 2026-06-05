// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::executor::block_on;
use gwr_components::queue::{Queue, QueueCore};
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::{connect_port, option_box_repeat};
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::Event;
use gwr_track::entity::GetEntity;

#[test]
fn queue_rejects_zero_capacity() {
    let engine = start_test(file!());

    let err = QueueCore::<usize>::new(engine.top(), "queue", Some(0)).unwrap_err();

    assert!(
        format!("{err}").contains("Unsupported Queue with 0 capacity"),
        "Unexpected error message: {err}"
    );
}

#[test]
fn queue_push_pop_fifo_and_reports_state() {
    let engine = start_test(file!());
    let queue = QueueCore::new(engine.top(), "queue", Some(2)).unwrap();

    assert!(queue.is_empty());
    assert!(!queue.is_full());
    assert_eq!(queue.len(), 0);
    assert_eq!(queue.values(), Vec::<usize>::new());

    block_on(queue.push(10)).unwrap();
    block_on(queue.push(20)).unwrap();

    assert!(!queue.is_empty());
    assert!(queue.is_full());
    assert_eq!(queue.len(), 2);
    assert_eq!(queue.values(), vec![10, 20]);

    assert_eq!(queue.pop_front(), Some(10));
    assert_eq!(queue.values(), vec![20]);
    assert_eq!(queue.pop_front(), Some(20));
    assert_eq!(queue.pop_front(), None);
    assert!(queue.is_empty());
    assert!(!queue.is_full());
}

#[test]
fn queue_with_no_capacity_limit_accepts_many_values() {
    const NUM_VALUES: usize = 1_000;

    let engine = start_test(file!());
    let queue = QueueCore::new(engine.top(), "queue", None).unwrap();

    for value in 0..NUM_VALUES {
        block_on(queue.push(value)).unwrap();
        assert!(!queue.is_full());
    }

    assert_eq!(queue.len(), NUM_VALUES);
    assert_eq!(queue.values(), (0..NUM_VALUES).collect::<Vec<_>>());
}

#[test]
fn queue_remove_where_removes_first_match() {
    let engine = start_test(file!());
    let queue = QueueCore::new(engine.top(), "queue", Some(4)).unwrap();

    block_on(queue.push(1)).unwrap();
    block_on(queue.push(2)).unwrap();
    block_on(queue.push(3)).unwrap();
    block_on(queue.push(2)).unwrap();

    assert_eq!(queue.remove_where(|value| *value == 2), Some(2));
    assert_eq!(queue.values(), vec![1, 3, 2]);

    assert_eq!(queue.remove_where(|value| *value == 99), None);
    assert_eq!(queue.values(), vec![1, 3, 2]);
}

#[test]
fn bounded_queue_push_waits_until_space_is_available() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let queue = Rc::new(QueueCore::new(engine.top(), "queue", Some(1)).unwrap());

    block_on(queue.push(1)).unwrap();

    let push_started = Rc::new(Cell::new(false));
    let push_done = Rc::new(Cell::new(false));

    {
        let push_started = push_started.clone();
        let push_done = push_done.clone();
        let queue = queue.clone();
        engine.spawn(async move {
            push_started.set(true);
            queue.push(2).await?;
            push_done.set(true);
            Ok(())
        });
    }

    {
        let clock = clock.clone();
        let push_started = push_started.clone();
        let push_done = push_done.clone();
        let queue = queue.clone();
        engine.spawn(async move {
            clock.wait_ticks(1).await;
            assert!(push_started.get());
            assert!(!push_done.get());
            assert_eq!(queue.values(), vec![1]);
            assert_eq!(queue.pop_front(), Some(1));
            Ok(())
        });
    }

    run_simulation!(engine);

    assert!(push_done.get());
    assert_eq!(queue.values(), vec![2]);
    assert_eq!(clock.time_now_ns(), 1.0);
}

#[test]
fn queue_changed_event_fires_for_mutations() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let queue = Rc::new(QueueCore::new(engine.top(), "queue", Some(2)).unwrap());
    let snapshots = Rc::new(RefCell::new(Vec::new()));

    {
        let changed = queue.changed_event();
        let queue = queue.clone();
        let snapshots = snapshots.clone();
        engine.spawn(async move {
            for _ in 0..4 {
                changed.listen().await;
                snapshots.borrow_mut().push(queue.values());
            }
            Ok(())
        });
    }

    {
        let clock = clock.clone();
        let queue = queue.clone();
        engine.spawn(async move {
            clock.wait_ticks(1).await;
            queue.push(10).await?;

            clock.wait_ticks(1).await;
            queue.push(20).await?;

            clock.wait_ticks(1).await;
            assert_eq!(queue.pop_front(), Some(10));

            clock.wait_ticks(1).await;
            assert_eq!(queue.remove_where(|value| *value == 20), Some(20));

            Ok(())
        });
    }

    run_simulation!(engine);

    assert_eq!(
        *snapshots.borrow(),
        vec![vec![10], vec![10, 20], vec![20], Vec::<usize>::new()]
    );
    assert_eq!(clock.time_now_ns(), 4.0);
}

#[test]
fn queue_component_connects_rx_to_tx_ports() {
    const NUM_VALUES: usize = 10;

    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let top = engine.top();

    let source =
        Source::new_and_register(&engine, top, "source", option_box_repeat!(5 ; NUM_VALUES))
            .unwrap();
    let queue = Queue::new_and_register(&engine, &clock, top, "queue", Some(2)).unwrap();
    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();

    connect_port!(source, tx => queue, rx).unwrap();
    connect_port!(queue, tx => sink, rx).unwrap();

    run_simulation!(engine);

    assert_eq!(sink.num_sunk(), NUM_VALUES);
    assert!(queue.is_empty());
}
