// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Private dense FP32 kernel; buffers are packed from completed simulated
//! loads.

#[cfg(target_os = "macos")]
const CBLAS_ROW_MAJOR: i32 = 101;
#[cfg(target_os = "macos")]
const CBLAS_NO_TRANSPOSE: i32 = 111;

pub(super) fn multiply(
    row_count: usize,
    column_count: usize,
    reduction_count: usize,
    left: &[f32],
    right: &[f32],
) -> Result<Vec<f32>, &'static str> {
    let (Some(left_len), Some(right_len), Some(output)) = (
        row_count.checked_mul(reduction_count),
        reduction_count.checked_mul(column_count),
        row_count.checked_mul(column_count),
    ) else {
        return Err("functional GEMM dimensions overflow");
    };
    if row_count == 0
        || column_count == 0
        || reduction_count == 0
        || left.len() != left_len
        || right.len() != right_len
    {
        return Err("functional GEMM buffer lengths do not match positive dimensions");
    }
    #[cfg(target_os = "macos")]
    let [Ok(rows), Ok(columns), Ok(reduction)] =
        [row_count, column_count, reduction_count].map(i32::try_from)
    else {
        return Err("functional GEMM dimensions exceed native BLAS integer range");
    };
    let mut result = vec![0.0; output];
    #[cfg(target_os = "macos")]
    // SAFETY: All dimensions are positive and fit C int; checked products match
    // exact slice lengths. Row-major leading dimensions are K/N/N. Owned output
    // cannot overlap either borrowed input; BLAS accesses no Rust interior state.
    unsafe {
        cblas_sgemm(
            CBLAS_ROW_MAJOR,
            CBLAS_NO_TRANSPOSE,
            CBLAS_NO_TRANSPOSE,
            rows,
            columns,
            reduction,
            1.0,
            left.as_ptr(),
            reduction,
            right.as_ptr(),
            columns,
            0.0,
            result.as_mut_ptr(),
            columns,
        );
    }
    #[cfg(not(target_os = "macos"))]
    portable(
        row_count,
        column_count,
        reduction_count,
        left,
        right,
        &mut result,
    );
    Ok(result)
}

#[cfg(target_os = "macos")]
#[link(name = "Accelerate", kind = "framework")]
unsafe extern "C" {
    fn cblas_sgemm(
        order: i32,
        trans_a: i32,
        trans_b: i32,
        m: i32,
        n: i32,
        k: i32,
        alpha: f32,
        a: *const f32,
        lda: i32,
        b: *const f32,
        ldb: i32,
        beta: f32,
        c: *mut f32,
        ldc: i32,
    );
}

#[cfg(any(not(target_os = "macos"), test))]
fn portable(
    row_count: usize,
    column_count: usize,
    reduction_count: usize,
    left: &[f32],
    right: &[f32],
    output: &mut [f32],
) {
    // Blocking retains ascending K accumulation and contiguous SIMD-friendly
    // columns while reusing each B panel across multiple output rows.
    for rows in (0..row_count).step_by(32) {
        for reduction in (0..reduction_count).step_by(64) {
            for columns in (0..column_count).step_by(128) {
                for row in rows..(rows + 32).min(row_count) {
                    let destination = &mut output[row * column_count..(row + 1) * column_count];
                    for inner in reduction..(reduction + 64).min(reduction_count) {
                        let value = left[row * reduction_count + inner];
                        let source = &right[inner * column_count..(inner + 1) * column_count];
                        for column in columns..(columns + 128).min(column_count) {
                            destination[column] += value * source[column];
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_buffers_and_overflow_before_native_access() {
        assert!(multiply(2, 3, 4, &[0.0; 7], &[0.0; 12]).is_err());
        assert!(multiply(2, 3, 4, &[0.0; 8], &[0.0; 11]).is_err());
        assert!(multiply(usize::MAX, 2, 2, &[], &[]).is_err());
        assert!(multiply(0, 3, 2, &[], &[0.0; 6]).is_err());
    }

    #[test]
    fn native_and_portable_match_independent_f64_for_rectangular_cancellation() {
        for (m, n, k) in [(17, 35, 129), (3, 7, 4097)] {
            let a = (0..m * k)
                .map(|i| ((i * 17 % 97) as f32 - 48.0) / 63.7)
                .collect::<Vec<_>>();
            let b = (0..k * n)
                .map(|i| ((i * 31 % 89) as f32 - 44.0) / 31.3)
                .collect::<Vec<_>>();
            let actual = multiply(m, n, k, &a, &b).unwrap();
            let mut fallback = vec![0.0; m * n];
            portable(m, n, k, &a, &b, &mut fallback);
            for row in 0..m {
                for column in 0..n {
                    let expected = (0..k)
                        .map(|index| {
                            f64::from(a[row * k + index]) * f64::from(b[index * n + column])
                        })
                        .sum::<f64>();
                    for &value in &[actual[row * n + column], fallback[row * n + column]] {
                        assert!(
                            (f64::from(value) - expected).abs() <= 1e-5 + 1e-5 * expected.abs()
                        );
                    }
                }
            }
        }
    }
}
