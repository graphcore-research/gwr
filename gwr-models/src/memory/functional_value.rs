// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Typed logical values retain exact storage bits, including NaN payloads and
//! signed/unsigned integers wider than the host floating-point significand.

use std::cmp::Ordering::{Equal, Greater, Less};

use gwr_engine::types::SimError;

use crate::processing_element::tensors::dtype::DataType;

/// One numerical value and its exact logical storage encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FunctionalValue {
    dtype: DataType,
    bits: u64,
}

impl FunctionalValue {
    /// Construct raw storage bits. Bits outside the declared width are errors.
    pub fn from_bits(dtype: DataType, bits: u64) -> Result<Self, SimError> {
        if bits & !mask(dtype) != 0 {
            return Err(SimError(format!("bits exceed {dtype:?} storage width")));
        }
        Ok(Self { dtype, bits })
    }

    /// Construct a signed integer, rejecting an out-of-range value.
    pub fn from_i64(dtype: DataType, value: i64) -> Result<Self, SimError> {
        if !signed(dtype)
            || ((value as i128) < -(1i128 << (dtype.num_bits() - 1)))
            || (value as i128) >= (1i128 << (dtype.num_bits() - 1))
        {
            return Err(SimError(format!("{value} is outside {dtype:?}")));
        }
        Ok(Self {
            dtype,
            bits: value as u64 & mask(dtype),
        })
    }

    /// Construct an unsigned integer, rejecting an out-of-range value.
    pub fn from_u64(dtype: DataType, value: u64) -> Result<Self, SimError> {
        if !unsigned(dtype) || value > mask(dtype) {
            return Err(SimError(format!("{value} is outside {dtype:?}")));
        }
        Ok(Self { dtype, bits: value })
    }

    /// Encode a floating value using round-to-nearest, ties-to-even.
    /// Fp8 uses E4M3FN; Fp4 uses E2M1. Both saturate overflow/infinity
    /// to the largest finite magnitude. E4M3FN supports NaN; E2M1 rejects it.
    /// FP16/BF16 retain IEEE infinity and quiet NaN semantics.
    pub fn from_f32(dtype: DataType, value: f32) -> Result<Self, SimError> {
        let bits = match dtype {
            DataType::Fp32 => value.to_bits() as u64,
            DataType::Bf16 => {
                let raw = value.to_bits();
                if value.is_nan() {
                    ((raw >> 16) | 0x40) as u64
                } else {
                    (raw.wrapping_add(0x7fff + ((raw >> 16) & 1)) >> 16) as u64
                }
            }
            DataType::Fp16 => encode_binary(value, 5, 10, 15) as u64,
            DataType::Fp8 => encode_e4m3fn(value) as u64,
            DataType::Fp4 => encode_e2m1(value)? as u64,
            _ => {
                return Err(SimError(format!(
                    "numeric floating encoding is unspecified for {dtype:?}"
                )));
            }
        };
        Ok(Self { dtype, bits })
    }

    /// Construct a packed one-bit boolean.
    #[must_use]
    pub fn from_bool(value: bool) -> Self {
        Self {
            dtype: DataType::Bool,
            bits: u64::from(value),
        }
    }

    /// Declared logical dtype.
    #[must_use]
    pub fn dtype(self) -> DataType {
        self.dtype
    }
    /// Exact storage encoding, with unused high bits zero.
    #[must_use]
    pub fn bits(self) -> u64 {
        self.bits
    }

    /// Read a signed integer without a floating-point intermediate.
    pub fn as_i64(self) -> Result<i64, SimError> {
        if !signed(self.dtype) {
            return Err(SimError("value is not signed integer".into()));
        }
        let shift = 64 - self.dtype.num_bits();
        Ok((self.bits << shift).cast_signed() >> shift)
    }

    /// Read an unsigned integer without a floating-point intermediate.
    pub fn as_u64(self) -> Result<u64, SimError> {
        if !unsigned(self.dtype) {
            return Err(SimError("value is not unsigned integer".into()));
        }
        Ok(self.bits)
    }

    /// Decode FP32, BF16, IEEE FP16, E4M3FN (Fp8), or E2M1 (Fp4).
    pub fn as_f32(self) -> Result<f32, SimError> {
        match self.dtype {
            DataType::Fp32 => Ok(f32::from_bits(self.bits as u32)),
            DataType::Bf16 => Ok(f32::from_bits((self.bits as u32) << 16)),
            DataType::Fp16 => Ok(decode_binary(self.bits as u32, 5, 10, 15)),
            DataType::Fp8 => Ok(decode_e4m3fn(self.bits as u8)),
            DataType::Fp4 => Ok(decode_e2m1(self.bits as u8)),
            _ => Err(SimError(format!(
                "numeric floating encoding is unspecified for {:?}",
                self.dtype
            ))),
        }
    }

    /// Read a one-bit boolean.
    pub fn as_bool(self) -> Result<bool, SimError> {
        if self.dtype != DataType::Bool {
            return Err(SimError("value is not boolean".into()));
        }
        Ok(self.bits != 0)
    }

    /// Explicit numerical conversion. Integer casts wrap modulo the target
    /// width. Floating-to-integer conversion truncates and saturates (NaN=0).
    /// Integer-to-floating conversion rounds directly from the exact integer.
    pub fn convert(self, dtype: DataType) -> Result<Self, SimError> {
        if dtype == self.dtype {
            return Ok(self);
        }
        if dtype == DataType::Bool {
            return Ok(Self::from_bool(if floating(self.dtype) {
                self.as_f32()? != 0.
            } else {
                self.bits != 0
            }));
        }
        if signed(dtype) || unsigned(dtype) {
            let bits = if floating(self.dtype) {
                let value = self.as_f32()?;
                if signed(dtype) {
                    let width = dtype.num_bits();
                    let min = -(1i128 << (width - 1));
                    let max = (1i128 << (width - 1)) - 1;
                    (value as i128).clamp(min, max) as u64
                } else {
                    (value as u128).min(mask(dtype) as u128) as u64
                }
            } else if signed(self.dtype) {
                self.as_i64()? as u64
            } else {
                self.bits
            };
            return Ok(Self {
                dtype,
                bits: bits & mask(dtype),
            });
        }
        if floating(dtype) {
            if floating(self.dtype) {
                return Self::from_f32(dtype, self.as_f32()?);
            }
            // Integer conversion must round once to the destination, rather
            // than through f32 first (which can double-round FP16/BF16 ties).
            let (negative, magnitude) = if signed(self.dtype) {
                let value = self.as_i64()?;
                (value < 0, value.unsigned_abs())
            } else {
                (false, self.bits)
            };
            let (eb, mb, bias) = match dtype {
                DataType::Fp32 => (8, 23, 127),
                DataType::Fp16 => (5, 10, 15),
                DataType::Bf16 => (8, 7, 127),
                DataType::Fp8 => (4, 3, 7),
                DataType::Fp4 => (2, 1, 1),
                _ => {
                    return Err(SimError(format!(
                        "numeric floating encoding is unspecified for {dtype:?}"
                    )));
                }
            };
            return Ok(Self {
                dtype,
                bits: encode_integer(negative, magnitude, eb, mb, bias) as u64,
            });
        }
        Err(SimError(format!("unsupported conversion to {dtype:?}")))
    }

    pub(crate) fn reinterpret(self, dtype: DataType) -> Result<Self, SimError> {
        if dtype.num_bits() != self.dtype.num_bits() {
            return Err(SimError(
                "reinterpret requires equal logical bit widths".into(),
            ));
        }
        Self::from_bits(dtype, self.bits)
    }
}

pub(crate) fn signed(dtype: DataType) -> bool {
    matches!(
        dtype,
        DataType::Int64 | DataType::Int32 | DataType::Int16 | DataType::Int8 | DataType::Int4
    )
}
pub(crate) fn unsigned(dtype: DataType) -> bool {
    matches!(
        dtype,
        DataType::Uint64 | DataType::Uint32 | DataType::Uint16 | DataType::Uint8
    )
}
pub(crate) fn floating(dtype: DataType) -> bool {
    matches!(
        dtype,
        DataType::Fp32 | DataType::Fp16 | DataType::Bf16 | DataType::Fp8 | DataType::Fp4
    )
}
pub(crate) fn mask(dtype: DataType) -> u64 {
    u64::MAX >> (64 - dtype.num_bits())
}

fn rounded_shift(value: u64, shift: u32) -> u64 {
    if shift == 0 {
        return value;
    }
    if shift >= 64 {
        return 0;
    }
    let result = value >> shift;
    let remainder = value & ((1u64 << shift) - 1);
    let halfway = 1u64 << (shift - 1);
    result + u64::from(remainder > halfway || (remainder == halfway && result & 1 != 0))
}

fn encode_integer(negative: bool, magnitude: u64, eb: u32, mb: u32, bias: i32) -> u32 {
    let sign = u32::from(negative) << (eb + mb);
    if magnitude == 0 {
        return sign;
    }
    let exponent = magnitude.ilog2();
    let significand = if exponent > mb {
        rounded_shift(magnitude, exponent - mb)
    } else {
        magnitude << (mb - exponent)
    };
    let mut exponent = exponent.cast_signed() + bias;
    let significand = if significand >= (2u64 << mb) {
        exponent += 1;
        significand >> 1
    } else {
        significand
    };
    let encoding = ((exponent as u32) << mb) | (significand as u32 & ((1 << mb) - 1));
    if eb == 4 {
        return sign | encoding.min(0x7e);
    }
    if eb == 2 {
        return sign | encoding.min(7);
    }
    if exponent >= (1 << eb) - 1 {
        return sign | (((1 << eb) - 1) << mb);
    }
    sign | encoding
}

pub(crate) fn decode_binary(bits: u32, eb: u32, mb: u32, bias: i32) -> f32 {
    let sign = if bits & (1 << (eb + mb)) == 0 {
        1.
    } else {
        -1.
    };
    let exponent = (bits >> mb) & ((1 << eb) - 1);
    let fraction = bits & ((1 << mb) - 1);
    if exponent == (1 << eb) - 1 {
        return if fraction == 0 {
            sign * f32::INFINITY
        } else {
            f32::NAN
        };
    }
    if exponent == 0 {
        sign * (fraction as f32) * 2f32.powi(1 - bias - mb.cast_signed())
    } else {
        sign * (1. + fraction as f32 / (1u32 << mb) as f32)
            * 2f32.powi(exponent.cast_signed() - bias)
    }
}

fn encode_binary(value: f32, eb: u32, mb: u32, bias: i32) -> u32 {
    let raw = value.to_bits();
    let sign = (raw >> 31) << (eb + mb);
    let exponent = ((raw >> 23) & 255).cast_signed();
    let fraction = raw & 0x007f_ffff;
    let inf = ((1 << eb) - 1) << mb;
    if exponent == 255 {
        return sign | inf | if fraction != 0 { 1 << (mb - 1) } else { 0 };
    }
    if exponent == 0 && fraction == 0 {
        return sign;
    }
    let target = exponent.max(1) - 127 + bias;
    let significand = if exponent == 0 {
        fraction
    } else {
        fraction | 0x0080_0000
    } as u64;
    if target <= 0 {
        return sign | rounded_shift(significand, (24 - mb.cast_signed() - target) as u32) as u32;
    }
    let rounded = rounded_shift(significand, 23 - mb) as u32;
    let target = target + i32::from(rounded == (2 << mb));
    if target >= (1 << eb) - 1 {
        return sign | inf;
    }
    sign | ((target as u32) << mb) | (rounded & ((1 << mb) - 1))
}

/// The generic Fp8 convention is E4M3FN (bias 7, finite exponent 15,
/// signed zero, two NaN encodings, maximum finite magnitude 448).
pub(crate) fn decode_e4m3fn(bits: u8) -> f32 {
    if bits & 0x7f == 0x7f {
        return f32::NAN;
    }
    let sign = if bits & 0x80 == 0 { 1. } else { -1. };
    let exponent = (bits >> 3) & 15;
    let fraction = bits & 7;
    if exponent == 0 {
        sign * fraction as f32 * 2f32.powi(-9)
    } else {
        sign * (1. + fraction as f32 / 8.) * 2f32.powi(i32::from(exponent) - 7)
    }
}

/// Packed E2M1: all encodings are finite, maximum magnitude six.
pub(crate) fn decode_e2m1(bits: u8) -> f32 {
    let magnitude = [0., 0.5, 1., 1.5, 2., 3., 4., 6.][(bits & 7) as usize];
    if bits & 8 == 0 { magnitude } else { -magnitude }
}

fn encode_e4m3fn(value: f32) -> u8 {
    if value.is_nan() {
        return 0x7f;
    }
    nearest_finite(value, 0x7e, decode_e4m3fn) | ((value.to_bits() >> 24) as u8 & 0x80)
}

fn encode_e2m1(value: f32) -> Result<u8, SimError> {
    if value.is_nan() {
        return Err(SimError("E2M1 has no NaN encoding".into()));
    }
    Ok(nearest_finite(value, 7, decode_e2m1) | ((value.to_bits() >> 28) as u8 & 8))
}

fn nearest_finite(value: f32, max: u8, decode: fn(u8) -> f32) -> u8 {
    let value = value.abs() as f64;
    if value >= decode(max) as f64 {
        return max;
    }
    let mut best = 0;
    let mut error = f64::INFINITY;
    for bits in 0..=max {
        let difference = (value - decode(bits) as f64).abs();
        if difference < error || (difference == error && bits & 1 == 0) {
            best = bits;
            error = difference;
        }
    }
    best
}

/// Supported operation arities are checked before simulated traffic.
pub(crate) fn arity(op: &str) -> Option<usize> {
    Some(match op {
        "add" | "sub" | "mul" | "div" | "maximum" | "minimum" | "bitand" | "bitor" | "bitxor"
        | "shift_left" | "shift_right" | "equal" | "not_equal" | "less" | "less_equal"
        | "greater" | "greater_equal" => 2,
        "relu" | "neg" | "sqrt" | "abs" | "exp" | "log" | "bitnot" | "convert" | "reinterpret" => 1,
        "select" => 3,
        _ => return None,
    })
}

pub(crate) fn validate_operation(
    op: &str,
    inputs: &[DataType],
    result: DataType,
) -> Result<(), SimError> {
    let invalid = || {
        SimError(format!(
            "undefined functional {op} signature {inputs:?} -> {result:?}; use explicit convert for domain changes"
        ))
    };
    if arity(op) != Some(inputs.len()) {
        return Err(invalid());
    }
    if op == "convert" {
        return Ok(());
    }
    if op == "reinterpret" {
        return if inputs[0].num_bits() == result.num_bits() {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    if op == "select" {
        return if inputs[0] == DataType::Bool && inputs[1] == result && inputs[2] == result {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    if matches!(op, "shift_left" | "shift_right") {
        return if (signed(result) || unsigned(result))
            && inputs[0] == result
            && (signed(inputs[1]) || unsigned(inputs[1]))
        {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    let same_domain = inputs.iter().all(|&dtype| {
        (floating(dtype) && floating(inputs[0]))
            || (signed(dtype) && signed(inputs[0]))
            || (unsigned(dtype) && unsigned(inputs[0]))
            || (dtype == DataType::Bool && dtype == inputs[0])
    });
    if !same_domain {
        return Err(invalid());
    }
    if matches!(
        op,
        "equal" | "not_equal" | "less" | "less_equal" | "greater" | "greater_equal"
    ) {
        return if result == DataType::Bool {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    if matches!(op, "bitand" | "bitor" | "bitxor" | "bitnot") {
        return if !floating(result) && inputs.iter().all(|&dtype| dtype == result) {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    if matches!(op, "sqrt" | "exp" | "log") {
        return if floating(result) && floating(inputs[0]) {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    if result == DataType::Bool {
        return if matches!(op, "maximum" | "minimum") && inputs[0] == result {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    if (floating(result) && floating(inputs[0]))
        || (signed(result) && signed(inputs[0]))
        || (unsigned(result) && unsigned(inputs[0]))
    {
        Ok(())
    } else {
        Err(invalid())
    }
}

pub(crate) fn apply(
    op: &str,
    values: &[FunctionalValue],
    result: DataType,
) -> Result<FunctionalValue, SimError> {
    let inputs: [DataType; 3] =
        std::array::from_fn(|i| values.get(i).map_or(result, |v| v.dtype()));
    validate_operation(op, &inputs[..values.len().min(3)], result)?;
    if op == "convert" {
        return values[0].convert(result);
    }
    if op == "reinterpret" {
        return values[0].reinterpret(result);
    }
    if op == "select" {
        return Ok(values[if values[0].as_bool()? { 1 } else { 2 }]);
    }
    let a = values[0];
    let b = values.get(1).copied().unwrap_or(a);
    if matches!(
        op,
        "equal" | "not_equal" | "less" | "less_equal" | "greater" | "greater_equal"
    ) {
        return compare(op, a, b);
    }
    if matches!(
        op,
        "bitand" | "bitor" | "bitxor" | "bitnot" | "shift_left" | "shift_right"
    ) {
        return apply_bits(op, a, b, result);
    }
    if floating(result) {
        return FunctionalValue::from_f32(result, apply_float(op, a.as_f32()?, b.as_f32()?));
    }
    let bits = if signed(result) {
        apply_signed(op, a.as_i64()?, b.as_i64()?)?
    } else {
        apply_unsigned(op, a.bits, b.bits)?
    };
    FunctionalValue::from_bits(result, bits & mask(result))
}

fn compare(op: &str, a: FunctionalValue, b: FunctionalValue) -> Result<FunctionalValue, SimError> {
    let ordering = if floating(a.dtype) {
        a.as_f32()?.partial_cmp(&b.as_f32()?)
    } else if signed(a.dtype) {
        Some(a.as_i64()?.cmp(&b.as_i64()?))
    } else {
        Some(a.bits.cmp(&b.bits))
    };
    Ok(FunctionalValue::from_bool(match op {
        "equal" => ordering == Some(Equal),
        "not_equal" => ordering != Some(Equal),
        "less" => ordering == Some(Less),
        "less_equal" => matches!(ordering, Some(Less | Equal)),
        "greater" => ordering == Some(Greater),
        _ => matches!(ordering, Some(Greater | Equal)),
    }))
}

fn apply_bits(
    op: &str,
    a: FunctionalValue,
    b: FunctionalValue,
    result: DataType,
) -> Result<FunctionalValue, SimError> {
    let bits = match op {
        "bitand" => a.bits & b.bits,
        "bitor" => a.bits | b.bits,
        "bitxor" => a.bits ^ b.bits,
        "bitnot" => !a.bits,
        _ => {
            let shift = if signed(b.dtype) {
                u64::try_from(b.as_i64()?)
                    .map_err(|_| SimError("negative functional shift".into()))?
            } else {
                b.bits
            };
            if shift >= a.dtype.num_bits() as u64 {
                return Err(SimError("functional shift exceeds logical width".into()));
            }
            if op == "shift_left" {
                a.bits << shift
            } else if signed(a.dtype) {
                (a.as_i64()? >> shift) as u64
            } else {
                a.bits >> shift
            }
        }
    };
    FunctionalValue::from_bits(result, bits & mask(result))
}

fn apply_float(op: &str, a: f32, b: f32) -> f32 {
    match op {
        "add" => a + b,
        "sub" => a - b,
        "mul" => a * b,
        "div" => a / b,
        "maximum" => a.max(b),
        "minimum" => a.min(b),
        "relu" => a.max(0.),
        "neg" => -a,
        "abs" => a.abs(),
        "sqrt" => a.sqrt(),
        "exp" => a.exp(),
        "log" => a.ln(),
        _ => unreachable!("operation validated"),
    }
}

fn apply_signed(op: &str, a: i64, b: i64) -> Result<u64, SimError> {
    Ok((match op {
        "add" => a.wrapping_add(b),
        "sub" => a.wrapping_sub(b),
        "mul" => a.wrapping_mul(b),
        "div" => {
            if b == 0 {
                return Err(SimError("functional integer division by zero".into()));
            }
            a.wrapping_div(b)
        }
        "maximum" => a.max(b),
        "minimum" => a.min(b),
        "relu" => a.max(0),
        "neg" => a.wrapping_neg(),
        "abs" => a.wrapping_abs(),
        _ => unreachable!("operation validated"),
    }) as u64)
}

fn apply_unsigned(op: &str, a: u64, b: u64) -> Result<u64, SimError> {
    Ok(match op {
        "add" => a.wrapping_add(b),
        "sub" => a.wrapping_sub(b),
        "mul" => a.wrapping_mul(b),
        "div" => a
            .checked_div(b)
            .ok_or_else(|| SimError("functional integer division by zero".into()))?,
        "maximum" => a.max(b),
        "minimum" => a.min(b),
        "relu" | "abs" => a,
        "neg" => a.wrapping_neg(),
        _ => unreachable!("operation validated"),
    })
}

/// MX scale bytes use unsigned E8M0 rather than generic Fp8/E4M3FN.
/// Zero encodes 2^-127; 255 encodes NaN. There is no zero or infinity.
pub(crate) fn decode_e8m0(bits: u8) -> f32 {
    if bits == 255 {
        f32::NAN
    } else {
        2f32.powi(bits as i32 - 127)
    }
}
#[cfg(test)]
mod tests {
    use DataType as D;

    use super::*;

    #[test]
    fn e8m0_scale_encoding_is_distinct_from_e4m3fn_values() {
        assert_eq!(decode_e8m0(0), f32::MIN_POSITIVE / 2.);
        assert_eq!(decode_e8m0(127), 1.);
        assert_eq!(decode_e8m0(254), 2f32.powi(127));
        assert!(decode_e8m0(255).is_nan());
        assert_eq!(decode_e4m3fn(0), 0.);
        assert!(decode_e4m3fn(127).is_nan());
    }

    #[test]
    fn arithmetic_keeps_signedness_width_and_exact_64_bit_values() {
        let i = |x| FunctionalValue::from_i64(D::Int64, x).unwrap();
        let u = |x| FunctionalValue::from_u64(D::Uint64, x).unwrap();
        assert_eq!(
            apply("add", &[i(9_007_199_254_740_993), i(2)], D::Int64)
                .unwrap()
                .as_i64()
                .unwrap(),
            9_007_199_254_740_995
        );
        assert_eq!(
            apply("add", &[u(u64::MAX), u(1)], D::Uint64)
                .unwrap()
                .as_u64()
                .unwrap(),
            0
        );
        assert_eq!(
            apply("div", &[i(i64::MIN), i(-1)], D::Int64)
                .unwrap()
                .as_i64()
                .unwrap(),
            i64::MIN
        );
        assert_eq!(
            apply(
                "minimum",
                &[u(u64::MAX), u(9_007_199_254_740_993)],
                D::Uint64
            )
            .unwrap()
            .as_u64()
            .unwrap(),
            9_007_199_254_740_993
        );
        assert!(apply("div", &[i(1), i(0)], D::Int64).is_err());
        assert!(apply("sqrt", &[i(1)], D::Int64).is_err());
        assert!(apply("add", &[i(1), u(1)], D::Int64).is_err());
        assert_eq!(
            apply("shift_right", &[i(-8), i(2)], D::Int64)
                .unwrap()
                .as_i64()
                .unwrap(),
            -2
        );
        assert!(apply("shift_left", &[u(1), u(64)], D::Uint64).is_err());
    }

    #[test]
    fn bool_logic_and_comparisons_have_explicit_domains() {
        let b = FunctionalValue::from_bool;
        assert!(apply("add", &[b(true), b(true)], D::Bool).is_err());
        assert_eq!(
            apply("bitand", &[b(true), b(false)], D::Bool).unwrap(),
            b(false)
        );
        let u = |x| FunctionalValue::from_u64(D::Uint64, x).unwrap();
        assert_eq!(
            apply("greater", &[u(u64::MAX), u(u64::MAX - 1)], D::Bool).unwrap(),
            b(true)
        );
        assert_eq!(
            apply("select", &[b(false), u(1), u(u64::MAX)], D::Uint64).unwrap(),
            u(u64::MAX)
        );
    }
}
