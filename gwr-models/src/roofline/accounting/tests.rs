// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use super::*;

#[test]
fn fragmented_ranges_are_not_repacked() {
    let mut runs = split_range(16, 64, 8).unwrap();
    runs.extend(split_range(16, 64, 8).unwrap());
    assert_eq!(
        account(Operation::Read, &runs).unwrap(),
        Traffic {
            transactions: 2,
            payload: 32,
            request_wire: 16,
            response_wire: 48,
        }
    );
}

#[test]
fn coalescing_preserves_headers_and_operation_semantics() {
    let mut runs = split_range(64, 64, 8).unwrap();
    runs.extend(split_range(64, 64, 8).unwrap());
    for (operation, request_wire, response_wire) in [
        (Operation::Read, 16, 144),
        (Operation::WriteNonPosted, 144, 16),
        (Operation::WritePosted, 144, 0),
    ] {
        assert_eq!(
            account(operation, &runs).unwrap(),
            Traffic {
                transactions: 2,
                payload: 128,
                request_wire,
                response_wire,
            }
        );
    }
}

#[test]
fn huge_ranges_keep_their_one_byte_tail_without_expansion() {
    for (bytes, transactions) in [
        (9_007_199_254_740_991, 140_737_488_355_328),
        (9_007_199_254_740_993, 140_737_488_355_329),
        (18_446_744_073_709_551_617, 288_230_376_151_711_745),
    ] {
        let runs = split_range(bytes, 64, 0).unwrap();
        assert!(runs.len() <= 2);
        assert_eq!(
            account(Operation::Read, &runs).unwrap(),
            Traffic {
                transactions,
                payload: bytes,
                request_wire: 0,
                response_wire: bytes,
            }
        );
    }
}

#[test]
fn empty_and_zero_maximum_are_distinct() {
    assert!(split_range(0, 64, 8).unwrap().is_empty());
    assert_eq!(account(Operation::Read, &[]).unwrap(), Traffic::default());
    assert_eq!(split_range(0, 0, 8), Err(AccountingError::ZeroMaximum));
}

#[test]
fn asymmetric_headers_are_charged_in_the_correct_direction() {
    let runs = [TransactionRun {
        payload_bytes: 16,
        count: 2,
        request_header_bytes: 4,
        response_header_bytes: 8,
    }];
    for (operation, request_wire, response_wire) in [
        (Operation::Read, 8, 48),
        (Operation::WriteNonPosted, 40, 16),
        (Operation::WritePosted, 40, 0),
    ] {
        assert_eq!(
            account(operation, &runs).unwrap(),
            Traffic {
                transactions: 2,
                payload: 32,
                request_wire,
                response_wire,
            }
        );
    }
}

#[test]
fn arithmetic_overflow_is_reported_not_wrapped() {
    for (payload_bytes, count, header) in [(u128::MAX, 2, 0), (u128::MAX, 1, 1), (1, 2, u128::MAX)]
    {
        let run = TransactionRun {
            payload_bytes,
            count,
            request_header_bytes: header,
            response_header_bytes: header,
        };
        assert_eq!(
            account(Operation::Read, &[run]),
            Err(AccountingError::Overflow)
        );
    }
    let run = TransactionRun {
        payload_bytes: 1,
        count: u128::MAX,
        request_header_bytes: 0,
        response_header_bytes: 0,
    };
    assert_eq!(
        account(Operation::Read, &[run, run]),
        Err(AccountingError::Overflow)
    );
}

#[test]
fn absent_transactions_and_posted_response_headers_do_not_overflow() {
    let run = TransactionRun {
        payload_bytes: u128::MAX,
        count: 0,
        request_header_bytes: u128::MAX,
        response_header_bytes: u128::MAX,
    };
    assert_eq!(
        account(Operation::Read, &[run]).unwrap(),
        Traffic::default()
    );
    let posted = TransactionRun {
        payload_bytes: 1,
        count: 2,
        request_header_bytes: 0,
        response_header_bytes: u128::MAX,
    };
    assert_eq!(
        account(Operation::WritePosted, &[posted]).unwrap(),
        Traffic {
            transactions: 2,
            payload: 2,
            request_wire: 2,
            response_wire: 0,
        }
    );
}
