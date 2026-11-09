// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::roofline::accounting::{Operation, Traffic, TransactionRun};
use gwr_models::roofline::plan::{DemandKind, ResourceUse, ServicePlan};

fn transaction(payload_bytes: u128, count: u128) -> TransactionRun {
    TransactionRun {
        payload_bytes,
        count,
        request_header_bytes: 8,
        response_header_bytes: 8,
    }
}

fn separate_resources() -> Vec<ResourceUse> {
    vec![
        ResourceUse {
            resource: 2,
            kind: DemandKind::Payload,
        },
        ResourceUse {
            resource: 0,
            kind: DemandKind::RequestWire,
        },
        ResourceUse {
            resource: 1,
            kind: DemandKind::ResponseWire,
        },
    ]
}

#[test]
fn operation_semantics_charge_payload_and_both_wire_directions_separately() {
    for (operation, q, r, expected) in [
        (Operation::Read, 8, 72, vec![(0, 8), (1, 72), (2, 64)]),
        (
            Operation::WriteNonPosted,
            72,
            8,
            vec![(0, 72), (1, 8), (2, 64)],
        ),
        (Operation::WritePosted, 72, 0, vec![(0, 72), (2, 64)]),
    ] {
        let plan = ServicePlan::new(
            operation,
            &[transaction(64, 1)],
            &separate_resources(),
            3,
            &[2, 3],
        )
        .unwrap();
        assert_eq!(
            plan.traffic(),
            Traffic {
                transactions: 1,
                payload: 64,
                request_wire: q,
                response_wire: r
            }
        );
        assert_eq!(plan.resource_demands(), expected);
        assert_eq!(plan.latency_ticks(), 5);
    }
}

#[test]
fn fragmented_reads_preserve_headers_and_shared_wire_capacity_sums_exactly() {
    let uses = [
        ResourceUse {
            resource: 0,
            kind: DemandKind::RequestWire,
        },
        ResourceUse {
            resource: 0,
            kind: DemandKind::ResponseWire,
        },
        ResourceUse {
            resource: 1,
            kind: DemandKind::Payload,
        },
    ];
    let plan = ServicePlan::new(
        Operation::Read,
        &[transaction(16, 1), transaction(16, 1)],
        &uses,
        2,
        &[],
    )
    .unwrap();
    assert_eq!(
        plan.traffic(),
        Traffic {
            transactions: 2,
            payload: 32,
            request_wire: 16,
            response_wire: 48
        }
    );
    assert_eq!(plan.resource_demands(), &[(0, 64), (1, 32)]);
}

#[test]
fn invalid_resource_ownership_and_indices_are_rejected_before_submission() {
    let payload = ResourceUse {
        resource: 0,
        kind: DemandKind::Payload,
    };
    assert!(
        ServicePlan::new(
            Operation::Read,
            &[transaction(64, 1)],
            &[payload, payload],
            1,
            &[]
        )
        .is_err()
    );
    assert!(ServicePlan::new(Operation::Read, &[transaction(64, 1)], &[payload], 0, &[]).is_err());
    // Even a posted write's absent response cannot refer to a missing resource.
    let response = ResourceUse {
        resource: 3,
        kind: DemandKind::ResponseWire,
    };
    assert!(
        ServicePlan::new(
            Operation::WritePosted,
            &[transaction(64, 1)],
            &[payload, response],
            1,
            &[]
        )
        .is_err()
    );
}

#[test]
fn zero_payload_header_work_and_unbounded_payload_are_rejected() {
    assert!(
        ServicePlan::new(
            Operation::Read,
            &[transaction(0, 1)],
            &separate_resources(),
            3,
            &[]
        )
        .is_err()
    );
    assert!(ServicePlan::new(Operation::Read, &[transaction(64, 1)], &[], 0, &[]).is_err());
    let response = ResourceUse {
        resource: 0,
        kind: DemandKind::ResponseWire,
    };
    assert!(
        ServicePlan::new(
            Operation::WritePosted,
            &[transaction(64, 1)],
            &[response],
            1,
            &[]
        )
        .is_err()
    );
    let empty = ServicePlan::new(Operation::Read, &[], &[], 0, &[2]).unwrap();
    assert_eq!(empty.traffic(), Traffic::default());
    assert!(empty.resource_demands().is_empty());
}

#[test]
fn arithmetic_overflow_in_accounting_resource_sums_and_latency_is_rejected() {
    assert!(
        ServicePlan::new(
            Operation::Read,
            &[transaction(u128::MAX, 2)],
            &separate_resources(),
            3,
            &[]
        )
        .is_err()
    );
    let huge = TransactionRun {
        payload_bytes: u128::MAX - 1,
        count: 1,
        request_header_bytes: 1,
        response_header_bytes: 1,
    };
    let shared = [
        ResourceUse {
            resource: 0,
            kind: DemandKind::RequestWire,
        },
        ResourceUse {
            resource: 0,
            kind: DemandKind::ResponseWire,
        },
    ];
    assert!(ServicePlan::new(Operation::Read, &[huge], &shared, 1, &[]).is_err());
    assert!(
        ServicePlan::new(
            Operation::Read,
            &[transaction(64, 1)],
            &separate_resources(),
            3,
            &[u64::MAX, 1]
        )
        .is_err()
    );
}

#[test]
fn exact_large_byte_totals_survive_resource_planning() {
    let plan = ServicePlan::new(
        Operation::Read,
        &[transaction(9_007_199_254_740_993, 1)],
        &separate_resources(),
        3,
        &[],
    )
    .unwrap();
    assert_eq!(
        plan.resource_demands(),
        &[
            (0, 8),
            (1, 9_007_199_254_741_001),
            (2, 9_007_199_254_740_993)
        ]
    );
    assert_eq!(plan.traffic().response_wire, 9_007_199_254_741_001);
}
