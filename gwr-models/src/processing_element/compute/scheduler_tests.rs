// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use crate::processing_element::compute::{
    CapabilityConfig, CapabilityKind, ComputeConfig, ComputeModel, ComputeScheduler,
    ExecutionUnitConfig, ExecutionUnitId, OperationClass,
};
use crate::processing_element::tensors::dtype::DataType;

fn two_execution_unit_scheduler() -> ComputeScheduler {
    let execution_unit = || ExecutionUnitConfig {
        matrix_accumulator_capacity_bytes: None,
        capabilities: vec![CapabilityConfig {
            kind: CapabilityKind::Scalar,
            class: "add".into(),
            input_dtypes: Some(vec![vec![DataType::Fp32; 2]]),
            ops_per_tick: 100.0,
        }],
    };
    ComputeScheduler::new(&Rc::new(
        ComputeModel::new(
            &ComputeConfig {
                execution_units: BTreeMap::from([
                    ("a".into(), execution_unit()),
                    ("b".into(), execution_unit()),
                ]),
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

fn poll_future_once<T: Future + Unpin>(future: &mut T) -> Poll<T::Output> {
    Pin::new(future).poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn same_execution_unit_acquisitions_wait_while_other_execution_units_remain_available() {
    let scheduler = two_execution_unit_scheduler();
    let mut first = scheduler.acquire(&[(ExecutionUnitId(0), 1)], 0);
    let Poll::Ready(Ok(guard)) = poll_future_once(&mut first) else {
        panic!("first acquisition")
    };
    let mut same_execution_unit = scheduler.acquire(&[(ExecutionUnitId(0), 1)], 0);
    assert!(poll_future_once(&mut same_execution_unit).is_pending());
    let mut other_execution_unit = scheduler.acquire(&[(ExecutionUnitId(1), 1)], 0);
    assert!(poll_future_once(&mut other_execution_unit).is_ready());
    drop(guard);
    assert!(poll_future_once(&mut same_execution_unit).is_ready());
}

#[test]
fn acquisition_uses_available_replicas_and_their_service_durations() {
    let scheduler = two_execution_unit_scheduler();
    let mut first =
        Box::pin(scheduler.acquire(&[(ExecutionUnitId(1), 3), (ExecutionUnitId(0), 7)], 0));
    let Poll::Ready(Ok(first_guard)) = poll_future_once(&mut first) else {
        panic!("first replica")
    };
    let mut second =
        Box::pin(scheduler.acquire(&[(ExecutionUnitId(1), 3), (ExecutionUnitId(0), 7)], 0));
    let Poll::Ready(Ok(second_guard)) = poll_future_once(&mut second) else {
        panic!("second replica")
    };
    assert_eq!(first_guard.execution_unit(), ExecutionUnitId(1));
    assert_eq!(first_guard.ticks(), 3);
    assert_eq!(second_guard.execution_unit(), ExecutionUnitId(0));
    assert_eq!(second_guard.ticks(), 7);
    let mut third =
        Box::pin(scheduler.acquire(&[(ExecutionUnitId(1), 3), (ExecutionUnitId(0), 7)], 0));
    assert!(poll_future_once(&mut third).is_pending());
    drop(first_guard);
    assert!(poll_future_once(&mut third).is_ready());
    drop(second_guard);
}

#[test]
fn cancelling_a_granted_waiter_releases_its_execution_unit() {
    let scheduler = two_execution_unit_scheduler();
    let mut holder_request = scheduler.acquire(&[(ExecutionUnitId(0), 1)], 0);
    let Poll::Ready(Ok(guard)) = poll_future_once(&mut holder_request) else {
        panic!("first acquisition")
    };
    let mut granted_waiter = scheduler.acquire(&[(ExecutionUnitId(0), 1)], 0);
    assert!(poll_future_once(&mut granted_waiter).is_pending());
    assert!(poll_future_once(&mut granted_waiter).is_pending());
    drop(guard);
    drop(granted_waiter);
    let mut next_request = scheduler.acquire(&[(ExecutionUnitId(0), 1)], 0);
    assert!(poll_future_once(&mut next_request).is_ready());
}

#[test]
fn blocked_execution_unit_does_not_block_another_execution_unit() {
    let scheduler = two_execution_unit_scheduler();
    let mut holder_request = scheduler.acquire(&[(ExecutionUnitId(0), 1)], 0);
    let Poll::Ready(Ok(holder_guard)) = poll_future_once(&mut holder_request) else {
        panic!("a")
    };
    let mut older_waiter = scheduler.acquire(&[(ExecutionUnitId(0), 1)], 0);
    let mut newer_waiter = scheduler.acquire(&[(ExecutionUnitId(0), 1)], 0);
    let mut independent_request = scheduler.acquire(&[(ExecutionUnitId(1), 1)], 0);
    assert!(poll_future_once(&mut older_waiter).is_pending());
    assert!(poll_future_once(&mut newer_waiter).is_pending());
    let Poll::Ready(Ok(independent_guard)) = poll_future_once(&mut independent_request) else {
        panic!("disjoint d blocked")
    };
    drop(holder_guard);
    let Poll::Ready(Ok(older_guard)) = poll_future_once(&mut older_waiter) else {
        panic!("older b")
    };
    assert!(poll_future_once(&mut newer_waiter).is_pending());
    drop(older_guard);
    assert!(poll_future_once(&mut newer_waiter).is_ready());
    drop(independent_guard);
}

#[test]
fn cancelled_waiter_does_not_hold_execution_unit() {
    let scheduler = two_execution_unit_scheduler();
    let mut holder_request = scheduler.acquire(&[(ExecutionUnitId(0), 1)], 0);
    let Poll::Ready(Ok(guard)) = poll_future_once(&mut holder_request) else {
        panic!("a")
    };
    let mut waiting = scheduler.acquire(&[(ExecutionUnitId(0), 1)], 0);
    assert!(poll_future_once(&mut waiting).is_pending());
    drop(waiting);
    let mut next = scheduler.acquire(&[(ExecutionUnitId(0), 1)], 0);
    assert!(poll_future_once(&mut next).is_pending());
    drop(guard);
    assert!(poll_future_once(&mut next).is_ready());
}

#[test]
fn an_unconsumed_flexible_grant_leaves_other_units_available() {
    let scheduler = two_execution_unit_scheduler();
    let mut first = scheduler.acquire(&[(ExecutionUnitId(0), 1)], 0);
    let mut second = scheduler.acquire(&[(ExecutionUnitId(1), 1)], 0);
    let Poll::Ready(Ok(first)) = poll_future_once(&mut first) else {
        panic!("first")
    };
    let Poll::Ready(Ok(second)) = poll_future_once(&mut second) else {
        panic!("second")
    };
    let mut flexible =
        Box::pin(scheduler.acquire(&[(ExecutionUnitId(0), 1), (ExecutionUnitId(1), 1)], 0));
    assert!(poll_future_once(&mut flexible).is_pending());
    drop(first);
    drop(second);
    // The flexible consumer has not polled again. Its grant must own only one
    // unit.
    let mut independent = scheduler.acquire(&[(ExecutionUnitId(1), 1)], 0);
    let Poll::Ready(Ok(independent)) = poll_future_once(&mut independent) else {
        panic!("one flexible request must leave the second unit available")
    };
    drop(flexible);
    let mut next = scheduler.acquire(&[(ExecutionUnitId(0), 1)], 0);
    assert!(poll_future_once(&mut next).is_ready());
    drop(independent);
}

#[test]
fn cancelling_a_queued_flexible_request_releases_every_alternative() {
    let scheduler = two_execution_unit_scheduler();
    let alternatives = [(ExecutionUnitId(0), 1), (ExecutionUnitId(1), 2)];
    let mut first = scheduler.acquire(&alternatives, 0);
    let mut second = scheduler.acquire(&alternatives, 0);
    let Poll::Ready(Ok(first)) = poll_future_once(&mut first) else {
        panic!("first")
    };
    let Poll::Ready(Ok(second)) = poll_future_once(&mut second) else {
        panic!("second")
    };
    let mut queued = scheduler.acquire(&alternatives, 0);
    assert!(poll_future_once(&mut queued).is_pending());
    assert!(poll_future_once(&mut queued).is_pending());
    drop(queued);
    drop((first, second));
    let mut next_first = scheduler.acquire(&alternatives, 0);
    let mut next_second = scheduler.acquire(&alternatives, 0);
    let Poll::Ready(Ok(next_first)) = poll_future_once(&mut next_first) else {
        panic!("first unit leaked")
    };
    let Poll::Ready(Ok(next_second)) = poll_future_once(&mut next_second) else {
        panic!("second unit leaked")
    };
    assert_ne!(next_first.execution_unit(), next_second.execution_unit());
}
