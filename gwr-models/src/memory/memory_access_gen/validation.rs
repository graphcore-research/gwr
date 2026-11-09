// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_engine::types::{SimError, SimResult};

use crate::memory::memory_map::MemoryMap;

/// Validate a finite arithmetic sequence of complete requests without
/// allocating or iterating one entry per request. Callers can repeat a
/// validated cycle.
pub(super) fn progression(
    map: &MemoryMap,
    base: u64,
    stride: u64,
    count: u128,
    bytes: usize,
) -> SimResult {
    if stride == 0 || bytes == 0 {
        return Err(SimError(
            "Generator stride and request size must be nonzero".into(),
        ));
    }
    if count == 0 {
        return Ok(());
    }
    let base = u128::from(base);
    let step = u128::from(stride);
    let last = base + (count - 1) * step;
    if last + bytes as u128 > u128::from(u64::MAX) + 1 {
        return Err(SimError(
            "Generator request range overflows address space".into(),
        ));
    }
    let mut covered = 0;
    for window in map.logical_ranges() {
        let first_index = window.start.saturating_sub(base).div_ceil(step);
        if first_index >= count {
            continue;
        }
        let first = base + first_index * step;
        if first >= window.end {
            continue;
        }
        let end_index = ((window.end - 1 - base) / step).min(count - 1);
        let end = base + end_index * step;
        map.resolve_access(first as u64, bytes)?;
        map.resolve_access(end as u64, bytes)?;
        if map.is_striped_address(first as u64) {
            map.validate_fetch_size(first as u64, bytes)?;
            if !first.is_multiple_of(bytes as u128)
                || (end_index > first_index && !step.is_multiple_of(bytes as u128))
            {
                return Err(SimError(
                    "Generator packet geometry may cross a stripe boundary".into(),
                ));
            }
        }
        covered += end_index - first_index + 1;
    }
    if covered != count {
        return Err(SimError(
            "Generator requests contain unmapped addresses".into(),
        ));
    }
    Ok(())
}
