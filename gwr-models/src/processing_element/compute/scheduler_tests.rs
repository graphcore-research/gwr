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
    let mut first = scheduler.acquire(ExecutionUnitId(0));
    let Poll::Ready(Ok(guard)) = poll_future_once(&mut first) else {
        panic!("first acquisition")
    };
    let mut same_execution_unit = scheduler.acquire(ExecutionUnitId(0));
    assert!(poll_future_once(&mut same_execution_unit).is_pending());
    let mut other_execution_unit = scheduler.acquire(ExecutionUnitId(1));
    assert!(poll_future_once(&mut other_execution_unit).is_ready());
    drop(guard);
    assert!(poll_future_once(&mut same_execution_unit).is_ready());
}

#[test]
fn acquire_any_uses_available_execution_unit_replicas() {
    let scheduler = two_execution_unit_scheduler();
    let mut first = Box::pin(scheduler.acquire_any(&[ExecutionUnitId(0), ExecutionUnitId(1)]));
    let Poll::Ready(Ok(first_guard)) = poll_future_once(&mut first) else {
        panic!("first replica")
    };
    let mut second = Box::pin(scheduler.acquire_any(&[ExecutionUnitId(0), ExecutionUnitId(1)]));
    let Poll::Ready(Ok(second_guard)) = poll_future_once(&mut second) else {
        panic!("second replica")
    };
    assert_ne!(first_guard.execution_unit(), second_guard.execution_unit());
    let mut third = Box::pin(scheduler.acquire_any(&[ExecutionUnitId(0), ExecutionUnitId(1)]));
    assert!(poll_future_once(&mut third).is_pending());
    drop(first_guard);
    assert!(poll_future_once(&mut third).is_ready());
    drop(second_guard);
}

#[test]
fn cancelling_a_granted_waiter_releases_its_execution_unit() {
    let scheduler = two_execution_unit_scheduler();
    let mut a = scheduler.acquire(ExecutionUnitId(0));
    let Poll::Ready(Ok(guard)) = poll_future_once(&mut a) else {
        panic!("first acquisition")
    };
    let mut b = scheduler.acquire(ExecutionUnitId(0));
    assert!(poll_future_once(&mut b).is_pending());
    assert!(poll_future_once(&mut b).is_pending());
    drop(guard);
    drop(b);
    let mut c = scheduler.acquire(ExecutionUnitId(0));
    assert!(poll_future_once(&mut c).is_ready());
}

#[test]
fn blocked_execution_unit_does_not_block_another_execution_unit() {
    let scheduler = two_execution_unit_scheduler();
    let mut a = scheduler.acquire(ExecutionUnitId(0));
    let Poll::Ready(Ok(a_guard)) = poll_future_once(&mut a) else {
        panic!("a")
    };
    let mut b = scheduler.acquire(ExecutionUnitId(0));
    let mut c = scheduler.acquire(ExecutionUnitId(0));
    let mut d = scheduler.acquire(ExecutionUnitId(1));
    assert!(poll_future_once(&mut b).is_pending());
    assert!(poll_future_once(&mut c).is_pending());
    let Poll::Ready(Ok(d_guard)) = poll_future_once(&mut d) else {
        panic!("disjoint d blocked")
    };
    drop(a_guard);
    let Poll::Ready(Ok(b_guard)) = poll_future_once(&mut b) else {
        panic!("older b")
    };
    assert!(poll_future_once(&mut c).is_pending());
    drop(b_guard);
    assert!(poll_future_once(&mut c).is_ready());
    drop(d_guard);
}

#[test]
fn cancelled_waiter_does_not_hold_execution_unit() {
    let scheduler = two_execution_unit_scheduler();
    let mut a = scheduler.acquire(ExecutionUnitId(0));
    let Poll::Ready(Ok(guard)) = poll_future_once(&mut a) else {
        panic!("a")
    };
    let mut waiting = scheduler.acquire(ExecutionUnitId(0));
    assert!(poll_future_once(&mut waiting).is_pending());
    drop(waiting);
    let mut next = scheduler.acquire(ExecutionUnitId(0));
    assert!(poll_future_once(&mut next).is_pending());
    drop(guard);
    assert!(poll_future_once(&mut next).is_ready());
}
