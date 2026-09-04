// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

/// Calculate
///
/// ```text
/// sum(i = 0..count) floor((step * i + offset) / modulus).
/// ```
///
/// Each iteration removes the whole multiples of `modulus` from `step` and
/// `offset`, then exchanges the reduced numerator and denominator. This is
/// the Euclidean reduction of the floor sum, so the number of iterations is
/// logarithmic in `step` and `modulus`.
pub(super) fn checked_floor_sum(
    mut count: u128,
    mut modulus: u128,
    mut step: u128,
    mut offset: u128,
) -> Option<u128> {
    if modulus == 0 {
        return None;
    }
    if count == 0 {
        return Some(0);
    }

    let mut total = 0u128;
    loop {
        if step >= modulus {
            let quotient = step / modulus;
            let triangular = if count.is_multiple_of(2) {
                (count / 2).checked_mul(count - 1)
            } else {
                count.checked_mul((count - 1) / 2)
            }?
            .checked_mul(quotient)?;
            total = total.checked_add(triangular)?;
            step %= modulus;
        }
        if offset >= modulus {
            total = total.checked_add(count.checked_mul(offset / modulus)?)?;
            offset %= modulus;
        }

        let maximum = step.checked_mul(count)?.checked_add(offset)?;
        if maximum < modulus {
            return Some(total);
        }
        count = maximum / modulus;
        offset = maximum % modulus;
        std::mem::swap(&mut modulus, &mut step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_sum_matches_direct_sum() {
        for count in 0..=12 {
            for modulus in 1..=12 {
                for step in 0..=12 {
                    for offset in 0..=12 {
                        let expected = (0..count)
                            .map(|index| (step * index + offset) / modulus)
                            .sum::<u128>();
                        assert_eq!(
                            checked_floor_sum(count, modulus, step, offset),
                            Some(expected),
                            "count={count}, modulus={modulus}, step={step}, offset={offset}"
                        );
                    }
                }
            }
        }
    }
}
