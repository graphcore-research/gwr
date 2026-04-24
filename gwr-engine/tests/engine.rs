// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_engine::types::SimResult;
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;

struct SelfWakingFuture {
    polls: Rc<Cell<usize>>,
}

impl Future for SelfWakingFuture {
    type Output = SimResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let poll_count = self.polls.get();
        self.polls.set(poll_count + 1);
        assert_eq!(poll_count, 0, "completed task was polled again");

        #[expect(clippy::waker_clone_wake)]
        cx.waker().clone().wake();
        Poll::Ready(Ok(()))
    }
}

#[test]
fn all_spawned() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let top = engine.top();
    let source: Rc<Source<i32>> = Source::new_and_register(&engine, top, "source", None).unwrap();
    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();

    source.connect_port_tx(sink.port_rx()).unwrap();
    run_simulation!(engine);
}

#[test]
fn self_wake_does_not_repoll_completed_task() {
    // It is essential that the engine does not poll a completed future. The easiest
    // way to ensure the duplicate poll doesn't happen is to create a poll
    // function that wakes itself.
    let mut engine = start_test(file!());
    let polls = Rc::new(Cell::new(0));

    engine.spawn(SelfWakingFuture {
        polls: polls.clone(),
    });

    run_simulation!(engine);
    assert_eq!(polls.get(), 1);
}

#[test]
fn randomized_task_order_uses_seeded_shuffle() {
    const TASKS: usize = 16;
    const SEED: u64 = 1234;

    let mut engine = start_test(file!());
    let order = Rc::new(RefCell::new(Vec::new()));

    engine.set_task_order_seed(SEED);
    engine.set_randomize_task_order(true);

    for task_id in 0..TASKS {
        let order = order.clone();
        engine.spawn(async move {
            order.borrow_mut().push(task_id);
            Ok(())
        });
    }

    run_simulation!(engine);

    let mut expected = (0..TASKS).collect::<Vec<_>>();
    let mut rng = StdRng::seed_from_u64(SEED);
    expected.shuffle(&mut rng);

    assert_eq!(*order.borrow(), expected);
    assert_ne!(*order.borrow(), (0..TASKS).collect::<Vec<_>>());
}
