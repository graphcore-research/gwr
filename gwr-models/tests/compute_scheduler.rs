// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use gwr_models::processing_element::compute::{
    CapabilityConfig, ComputeConfig, ComputeModel, ComputeScheduler, EngineConfig, EngineId,
    OperationClass,
};
use gwr_models::processing_element::tensors::dtype::DataType;

#[test]
fn batches_share_an_engine_but_distinct_engines_run_concurrently() {
    let scheduler = scheduler();
    let mut first = scheduler.acquire(EngineId(0));
    let Poll::Ready(Ok(guard)) = poll(&mut first) else {
        panic!("first acquisition")
    };
    let mut same_engine = scheduler.acquire(EngineId(0));
    assert!(poll(&mut same_engine).is_pending());
    let mut other_engine = scheduler.acquire(EngineId(1));
    assert!(poll(&mut other_engine).is_ready());
    drop(guard);
    assert!(poll(&mut same_engine).is_ready());
}

#[test]
fn equivalent_batches_choose_an_available_replica() {
    let scheduler = scheduler();
    let mut first = Box::pin(scheduler.acquire_any(&[EngineId(0), EngineId(1)]));
    let Poll::Ready(Ok(first_guard)) = poll(&mut first) else {
        panic!("first replica")
    };
    let mut second = Box::pin(scheduler.acquire_any(&[EngineId(0), EngineId(1)]));
    let Poll::Ready(Ok(second_guard)) = poll(&mut second) else {
        panic!("second replica")
    };
    assert_ne!(first_guard.engine(), second_guard.engine());
    let mut third = Box::pin(scheduler.acquire_any(&[EngineId(0), EngineId(1)]));
    assert!(poll(&mut third).is_pending());
    drop(first_guard);
    assert!(poll(&mut third).is_ready());
    drop(second_guard);
}

#[test]
fn cancellation_after_grant_releases_capacity() {
    let scheduler = scheduler();
    let mut a = scheduler.acquire(EngineId(0));
    let Poll::Ready(Ok(guard)) = poll(&mut a) else {
        panic!("first acquisition")
    };
    let mut b = scheduler.acquire(EngineId(0));
    assert!(poll(&mut b).is_pending());
    assert!(poll(&mut b).is_pending());
    drop(guard);
    drop(b);
    let mut c = scheduler.acquire(EngineId(0));
    assert!(poll(&mut c).is_ready());
}

#[test]
fn blocked_engine_does_not_block_another_engine() {
    let scheduler = scheduler();
    let mut a = scheduler.acquire(EngineId(0));
    let Poll::Ready(Ok(a_guard)) = poll(&mut a) else {
        panic!("a")
    };
    let mut b = scheduler.acquire(EngineId(0));
    let mut c = scheduler.acquire(EngineId(0));
    let mut d = scheduler.acquire(EngineId(1));
    assert!(poll(&mut b).is_pending());
    assert!(poll(&mut c).is_pending());
    let Poll::Ready(Ok(d_guard)) = poll(&mut d) else {
        panic!("disjoint d blocked")
    };
    drop(a_guard);
    let Poll::Ready(Ok(b_guard)) = poll(&mut b) else {
        panic!("older b")
    };
    assert!(poll(&mut c).is_pending());
    drop(b_guard);
    assert!(poll(&mut c).is_ready());
    drop(d_guard);
}

#[test]
fn cancelled_waiter_does_not_hold_engine() {
    let scheduler = scheduler();
    let mut a = scheduler.acquire(EngineId(0));
    let Poll::Ready(Ok(guard)) = poll(&mut a) else {
        panic!("a")
    };
    let mut waiting = scheduler.acquire(EngineId(0));
    assert!(poll(&mut waiting).is_pending());
    drop(waiting);
    let mut next = scheduler.acquire(EngineId(0));
    assert!(poll(&mut next).is_pending());
    drop(guard);
    assert!(poll(&mut next).is_ready());
}

fn scheduler() -> ComputeScheduler {
    let engine = || EngineConfig {
        matrix_accumulator_capacity_bytes: None,
        capabilities: vec![CapabilityConfig {
            matrix_service_tile: None,
            execution: None,
            class: "add".into(),
            input_dtypes: Some(vec![vec![DataType::Fp32; 2]]),
            ops_per_tick: 100.0,
        }],
    };
    ComputeScheduler::new(&Rc::new(
        ComputeModel::new(
            &ComputeConfig {
                engines: BTreeMap::from([("a".into(), engine()), ("b".into(), engine())]),
            },
            &BTreeMap::from([(
                "add".into(),
                OperationClass {
                    operations: vec!["add".into()],
                    arity: 2,
                    flops_per_op: 1,
                },
            )]),
        )
        .unwrap(),
    ))
}

fn poll<T: Future + Unpin>(future: &mut T) -> Poll<T::Output> {
    Pin::new(future).poll(&mut Context::from_waker(std::task::Waker::noop()))
}
