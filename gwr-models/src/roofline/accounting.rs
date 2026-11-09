// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Exact demand summaries. These do not replace address/provenance ledgers.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Read,
    WriteNonPosted,
    WritePosted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransactionRun {
    /// Payload and headers are per transaction, not totals for the run.
    pub payload_bytes: u128,
    pub count: u128,
    pub request_header_bytes: u128,
    pub response_header_bytes: u128,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Traffic {
    pub transactions: u128,
    pub payload: u128,
    pub request_wire: u128,
    pub response_wire: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountingError {
    ZeroMaximum,
    Overflow,
}

/// Split one physical range into at most two uniform runs, without packing
/// gaps.
pub fn split_range(
    bytes: u128,
    maximum: u128,
    header: u128,
) -> Result<Vec<TransactionRun>, AccountingError> {
    if maximum == 0 {
        return Err(AccountingError::ZeroMaximum);
    }
    let mut runs = Vec::with_capacity(2);
    for (payload_bytes, count) in [(maximum, bytes / maximum), (bytes % maximum, 1)] {
        if payload_bytes != 0 && count != 0 {
            runs.push(TransactionRun {
                payload_bytes,
                count,
                request_header_bytes: header,
                response_header_bytes: header,
            });
        }
    }
    Ok(runs)
}

/// Sum existing transactions without changing their shape or overhead.
pub fn account(operation: Operation, runs: &[TransactionRun]) -> Result<Traffic, AccountingError> {
    use AccountingError::Overflow;
    let mut result = Traffic::default();
    for run in runs {
        let payload = run.payload_bytes.checked_mul(run.count).ok_or(Overflow)?;
        let request_header = run
            .request_header_bytes
            .checked_mul(run.count)
            .ok_or(Overflow)?;
        let response_header = if operation == Operation::WritePosted {
            0
        } else {
            run.response_header_bytes
                .checked_mul(run.count)
                .ok_or(Overflow)?
        };
        let (request, response) = match operation {
            Operation::Read => (
                request_header,
                payload.checked_add(response_header).ok_or(Overflow)?,
            ),
            Operation::WriteNonPosted | Operation::WritePosted => (
                payload.checked_add(request_header).ok_or(Overflow)?,
                response_header,
            ),
        };
        result.transactions = result.transactions.checked_add(run.count).ok_or(Overflow)?;
        result.payload = result.payload.checked_add(payload).ok_or(Overflow)?;
        result.request_wire = result.request_wire.checked_add(request).ok_or(Overflow)?;
        result.response_wire = result.response_wire.checked_add(response).ok_or(Overflow)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
