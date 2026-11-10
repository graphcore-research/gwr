// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Reproducible entropy for the Linux timetable one-shot benchmark only.
//!
//! Rust's standard library and `rand` resolve the executable's `getrandom`
//! symbol, so their existing RNGs and hash maps receive repeatable seeds
//! without changing the simulator's collection types. Never include this module
//! in a normal application: its output is deliberately predictable.
//!
//! Repeatability requires the same entropy requests in the same order.
//! Concurrent callers and sources that bypass this symbol (such as mimalloc's
//! raw syscall) are not controlled by this hook.

use std::sync::{LazyLock, Mutex, PoisonError};

use rand::{Rng, SeedableRng};
use rand_xoshiro::SplitMix64;

// An explicit seed avoids recursively requesting OS entropy during
// initialization.
static RNG: LazyLock<Mutex<SplitMix64>> =
    LazyLock::new(|| Mutex::new(SplitMix64::seed_from_u64(0)));

/// Supply deterministic SplitMix64 words instead of OS entropy.
///
/// This benchmark hook accepts the flags used by Rust and `rand` without
/// changing its output. A zero-length request succeeds without consuming any
/// entropy.
///
/// # Safety
///
/// For nonzero `length`, `buffer` must be valid for writes of `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getrandom(buffer: *mut u8, length: usize, _flags: u32) -> isize {
    // Short reads are permitted by getrandom; keep the result representable.
    let length = length.min(isize::MAX as usize);
    if length == 0 {
        return 0;
    }

    let mut rng = RNG.lock().unwrap_or_else(PoisonError::into_inner);
    for offset in (0..length).step_by(8) {
        let bytes = rng.next_u64().to_le_bytes();
        let chunk_length = (length - offset).min(bytes.len());
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.add(offset), chunk_length);
        }
    }

    isize::try_from(length).unwrap_or(isize::MAX)
}
