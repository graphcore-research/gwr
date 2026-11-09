// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::types::{SimError, SimResult};

/// Nonphysical bounded lane-vector allocation limit, unrelated to capacity.
const MAX_MEMORY_SERVICE_LANES: usize = 256;

fn validate_lane_geometry(interleave: u64, count: usize) -> SimResult {
    if !interleave.is_power_of_two() || count == 0 || count > MAX_MEMORY_SERVICE_LANES {
        return Err(SimError(
            "memory lanes require power-of-two stripe and count within1..=256".into(),
        ));
    }
    usize::try_from(interleave).map_err(|e| SimError(e.to_string()))?;
    Ok(())
}

/// Conserved payload bytes per caller-declared relative modulo service lane.
/// Uses bounded lane storage and arithmetic full stripe cycles, never children
/// per byte/stripe. This is not a physical HBM address mapping certificate.
/// Zero accounting is allowed here only; real Memory intake rejects zero.
pub fn payload_by_lane(
    base: u64,
    address: u64,
    bytes: usize,
    interleave: u64,
    count: usize,
) -> Result<Vec<usize>, SimError> {
    validate_lane_geometry(interleave, count)?;
    let start = address
        .checked_sub(base)
        .ok_or_else(|| SimError("memory lane address below base".into()))?;
    if bytes != 0 {
        let size = u64::try_from(bytes).map_err(|e| SimError(e.to_string()))?;
        super::checked_last_address(address, size)
            .ok_or_else(|| SimError("memory lane payload range overflows".into()))?;
    }
    let start = u128::from(start);
    let end = start
        .checked_add(bytes as u128)
        .ok_or_else(|| SimError("memory lane range overflows".into()))?;
    let stripe = u128::from(interleave);
    let period = stripe
        .checked_mul(count as u128)
        .ok_or_else(|| SimError("memory lane period overflows".into()))?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(count)
        .map_err(|e| SimError(e.to_string()))?;
    for lane in 0..count {
        let lane_start = stripe * lane as u128;
        let prefix = |position: u128| -> Result<u128, SimError> {
            let complete = (position / period)
                .checked_mul(stripe)
                .ok_or_else(|| SimError("memory lane byte count overflows".into()))?;
            let partial = (position % period).saturating_sub(lane_start).min(stripe);
            complete
                .checked_add(partial)
                .ok_or_else(|| SimError("memory lane byte count overflows".into()))
        };
        let payload = prefix(end)?
            .checked_sub(prefix(start)?)
            .ok_or_else(|| SimError("memory lane count underflows".into()))?;
        result.push(usize::try_from(payload).map_err(|e| SimError(e.to_string()))?);
    }
    Ok(result)
}
