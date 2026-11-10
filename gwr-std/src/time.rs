// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
// Documentation adapted from Rust 1.98.1's Duration API documentation.
// Copyright (c) The Rust Project Contributors.
// The adapted documentation is covered by the MIT license in LICENSE.
// https://doc.rust-lang.org/std/time/struct.Duration.html

//! Time spans for simulations, including fractions of a nanosecond.
//!
//! [`Duration`] follows the conventions of [`std::time::Duration`], with
//! a `u32` seconds counter and `f64` subsecond nanoseconds. Numeric
//! constructors return durations directly, with fallible `try_*` variants. Its
//! API documentation is adapted from the Rust standard library documentation to
//! describe these differences.

use std::fmt::{self, Display};
use std::ops::{Add, Mul, Sub};

use crate::error::SimError;

const NANOS_PER_SEC: u64 = 1_000_000_000;
const NANOS_PER_SEC_F64: f64 = NANOS_PER_SEC as f64;
const NANOS_PER_MILLI: u128 = 1_000_000;
const NANOS_PER_MICRO: u128 = 1_000;

/// A nonnegative span of simulated time.
///
/// A duration stores whole seconds as a `u32` and subsecond nanoseconds as an
/// `f64`. The nanosecond component is finite, nonnegative, and less than
/// `1_000_000_000.0`. It can include fractions of a nanosecond, with precision
/// determined by `f64`. The supported range is [`ZERO`](Self::ZERO) through
/// [`MAX`](Self::MAX), approximately 136 years.
///
/// Numeric constructors panic for invalid or out-of-range inputs, except
/// [`from_secs`](Self::from_secs), whose `u32` inputs always fit. Their
/// `try_*` counterparts and [`from_nanos_str`](Self::from_nanos_str) return a
/// [`SimError`] instead.
/// [`Default`] produces [`ZERO`](Self::ZERO). The `+`, `-`, and `* u32`
/// operators panic on overflow or underflow; the `checked_*` and
/// `saturating_*` methods provide alternatives.
///
/// # Examples
///
/// ```
/// use gwr_std::time::Duration;
///
/// let five_seconds = Duration::from_secs(5);
/// let time = five_seconds + Duration::new(0, 0.125);
///
/// assert_eq!(time.as_secs(), 5);
/// assert_eq!(time.subsec_nanos(), 0.125);
/// assert_eq!(time.to_nanos_string(), "5000000000.125");
/// ```
///
/// # Precision and formatting
///
/// Integer constructors preserve whole nanoseconds exactly. Arithmetic on the
/// fractional component uses floating-point rounding. Converting an entire
/// duration to `f64` can lose precision at large times, while
/// [`as_nanos`](Self::as_nanos) discards fractions of a nanosecond. Use
/// [`to_nanos_string`](Self::to_nanos_string) for decimal timestamps that
/// preserve the stored value when parsed again.
///
/// [`Display`] selects seconds, milliseconds, microseconds (`us`), or
/// nanoseconds for human-readable output. See [`to_appropriate_unit_fmt`](
/// Self::to_appropriate_unit_fmt) for unit selection and precision rules.
#[derive(Copy, Clone, PartialEq, PartialOrd, Debug, Default)]
pub struct Duration {
    secs: u32,
    subsec_nanos: f64,
}

impl Duration {
    /// A zero-length time span, also returned by [`Default`].
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// assert_eq!(Duration::ZERO, Duration::default());
    /// assert_eq!(Duration::ZERO.as_nanos(), 0);
    /// ```
    pub const ZERO: Self = Self {
        secs: 0,
        subsec_nanos: 0.0,
    };

    /// The largest supported time span.
    ///
    /// Its seconds counter is [`u32::MAX`] and its subsecond component is the
    /// greatest `f64` less than `1_000_000_000.0` nanoseconds.
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// assert_eq!(Duration::MAX.as_secs(), u32::MAX);
    /// assert!(Duration::MAX.checked_add(Duration::from_secs(1)).is_none());
    /// ```
    pub const MAX: Self = Self {
        secs: u32::MAX,
        subsec_nanos: f64::from_bits(NANOS_PER_SEC_F64.to_bits() - 1),
    };

    /// Builds a duration from whole seconds and additional nanoseconds.
    ///
    /// Nanoseconds may be fractional. Whole seconds contained in `nanos` are
    /// carried into `secs`; negative zero is normalized to positive zero.
    ///
    /// # Panics
    ///
    /// Panics if `nanos` is negative, infinite, or NaN, or if
    /// carrying whole seconds would exceed [`MAX`](Self::MAX).
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::new(2, 1_000_000_000.25);
    /// assert_eq!(time.as_secs(), 3);
    /// assert_eq!(time.subsec_nanos(), 0.25);
    /// ```
    #[must_use]
    pub fn new(secs: u32, nanos: f64) -> Self {
        Self::try_new(secs, nanos).expect("invalid duration")
    }

    /// Builds a duration from whole seconds and additional nanoseconds.
    ///
    /// The fallible counterpart of [`new`](Self::new).
    ///
    /// Nanoseconds may be fractional. Whole seconds contained in `nanos` are
    /// carried into `secs`; negative zero is normalized to positive zero.
    ///
    /// # Errors
    ///
    /// Returns [`SimError`] if `nanos` is negative, infinite, or NaN, or if
    /// carrying whole seconds would exceed [`MAX`](Self::MAX).
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::try_new(2, 1_000_000_000.25).unwrap();
    /// assert_eq!(time.as_secs(), 3);
    /// assert_eq!(time.subsec_nanos(), 0.25);
    /// assert!(Duration::try_new(0, f64::NAN).is_err());
    /// ```
    pub fn try_new(secs: u32, nanos: f64) -> Result<Self, SimError> {
        Self::normalize(secs, nanos)
            .ok_or_else(|| SimError(format!("Invalid duration: {secs}s + {nanos}ns")))
    }

    /// Builds a duration containing an integer count of seconds.
    ///
    /// The subsecond component is zero. Every `u32` input fits, so this
    /// constructor cannot fail and can be used in constants.
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::from_secs(5);
    /// assert_eq!(time.as_secs(), 5);
    /// assert_eq!(time.subsec_nanos(), 0.0);
    /// ```
    #[must_use]
    pub const fn from_secs(secs: u32) -> Self {
        Self { secs, ..Self::ZERO }
    }

    /// Builds a duration containing an integer count of seconds.
    ///
    /// The fallible counterpart of [`from_secs`](Self::from_secs).
    ///
    /// Accepts a wider `u64` count and checks it against the `u32` seconds
    /// range. The subsecond component is zero.
    ///
    /// # Errors
    ///
    /// Returns [`SimError`] when `secs` exceeds [`u32::MAX`].
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::try_from_secs(5).unwrap();
    /// assert_eq!(time.as_secs(), 5);
    /// assert_eq!(time.subsec_nanos(), 0.0);
    /// ```
    pub fn try_from_secs(secs: u64) -> Result<Self, SimError> {
        let secs = u32::try_from(secs)
            .map_err(|_| SimError(format!("Duration seconds exceed u32::MAX: {secs}")))?;
        Ok(Self::from_secs(secs))
    }

    /// Builds a duration containing an integer count of milliseconds.
    ///
    /// # Panics
    ///
    /// Panics if the duration exceeds [`MAX`](Self::MAX).
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::from_millis(1_250);
    /// assert_eq!(time.as_secs(), 1);
    /// assert_eq!(time.subsec_nanos(), 250_000_000.0);
    /// ```
    #[must_use]
    pub fn from_millis(millis: u64) -> Self {
        Self::try_from_millis(millis).expect("duration milliseconds exceed the supported range")
    }

    /// Builds a duration containing an integer count of milliseconds.
    ///
    /// The fallible counterpart of [`from_millis`](Self::from_millis).
    ///
    /// # Errors
    ///
    /// Returns [`SimError`] if the duration exceeds [`MAX`](Self::MAX).
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::try_from_millis(1_250).unwrap();
    /// assert_eq!(time.as_secs(), 1);
    /// assert_eq!(time.subsec_nanos(), 250_000_000.0);
    /// ```
    pub fn try_from_millis(millis: u64) -> Result<Self, SimError> {
        Self::try_from_nanos_u128(u128::from(millis) * NANOS_PER_MILLI)
    }

    /// Builds a duration containing an integer count of microseconds.
    ///
    /// # Panics
    ///
    /// Panics if the duration exceeds [`MAX`](Self::MAX).
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::from_micros(1_000_125);
    /// assert_eq!(time.as_secs(), 1);
    /// assert_eq!(time.subsec_nanos(), 125_000.0);
    /// ```
    #[must_use]
    pub fn from_micros(micros: u64) -> Self {
        Self::try_from_micros(micros).expect("duration microseconds exceed the supported range")
    }

    /// Builds a duration containing an integer count of microseconds.
    ///
    /// The fallible counterpart of [`from_micros`](Self::from_micros).
    ///
    /// # Errors
    ///
    /// Returns [`SimError`] if the duration exceeds [`MAX`](Self::MAX).
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::try_from_micros(1_000_125).unwrap();
    /// assert_eq!(time.as_secs(), 1);
    /// assert_eq!(time.subsec_nanos(), 125_000.0);
    /// ```
    pub fn try_from_micros(micros: u64) -> Result<Self, SimError> {
        Self::try_from_nanos_u128(u128::from(micros) * NANOS_PER_MICRO)
    }

    /// Builds a duration containing an integer count of nanoseconds.
    ///
    /// The integer count is preserved exactly, including values above `2^53`.
    /// Use [`new`](Self::new) to provide fractional nanoseconds.
    ///
    /// # Panics
    ///
    /// Panics if the duration exceeds [`MAX`](Self::MAX).
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::from_nanos(1_000_000_123);
    /// assert_eq!(time.as_secs(), 1);
    /// assert_eq!(time.subsec_nanos(), 123.0);
    /// ```
    #[must_use]
    pub fn from_nanos(nanos: u64) -> Self {
        Self::try_from_nanos(nanos).expect("duration nanoseconds exceed the supported range")
    }

    /// Builds a duration containing an integer count of nanoseconds.
    ///
    /// The fallible counterpart of [`from_nanos`](Self::from_nanos).
    ///
    /// The integer count is preserved exactly, including values above `2^53`.
    /// Use [`new`](Self::new) to provide fractional nanoseconds.
    ///
    /// # Errors
    ///
    /// Returns [`SimError`] if the duration exceeds [`MAX`](Self::MAX).
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::try_from_nanos(1_000_000_123).unwrap();
    /// assert_eq!(time.as_secs(), 1);
    /// assert_eq!(time.subsec_nanos(), 123.0);
    /// ```
    pub fn try_from_nanos(nanos: u64) -> Result<Self, SimError> {
        Self::try_from_nanos_u128(u128::from(nanos))
    }

    /// Builds a duration from an integer nanosecond count supplied as a `u128`.
    ///
    /// This accepts the return type of [`as_nanos`](Self::as_nanos), with the
    /// same supported duration range as [`from_nanos`](Self::from_nanos).
    /// Fractions discarded by `as_nanos` cannot be recovered by this method.
    ///
    /// # Panics
    ///
    /// Panics if the duration exceeds [`MAX`](Self::MAX).
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let nanos = 9_007_199_254_740_993_u128;
    /// let time = Duration::from_nanos_u128(nanos);
    /// assert_eq!(time.as_nanos(), nanos);
    /// ```
    #[must_use]
    pub fn from_nanos_u128(nanos: u128) -> Self {
        Self::try_from_nanos_u128(nanos).expect("duration nanoseconds exceed the supported range")
    }

    /// Builds a duration from an integer nanosecond count supplied as a `u128`.
    ///
    /// The fallible counterpart of [`from_nanos_u128`](Self::from_nanos_u128).
    ///
    /// This accepts the return type of [`as_nanos`](Self::as_nanos), with the
    /// same supported duration range as [`from_nanos`](Self::from_nanos).
    /// Fractions discarded by `as_nanos` cannot be recovered by this method.
    ///
    /// # Errors
    ///
    /// Returns [`SimError`] if the duration exceeds [`MAX`](Self::MAX).
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let nanos = 9_007_199_254_740_993_u128;
    /// let time = Duration::try_from_nanos_u128(nanos).unwrap();
    /// assert_eq!(time.as_nanos(), nanos);
    /// ```
    pub fn try_from_nanos_u128(nanos: u128) -> Result<Self, SimError> {
        let secs = u32::try_from(nanos / u128::from(NANOS_PER_SEC)).map_err(|_| {
            SimError(format!(
                "Duration nanoseconds exceed the supported range: {nanos}"
            ))
        })?;
        Ok(Self {
            secs,
            subsec_nanos: (nanos % u128::from(NANOS_PER_SEC)) as f64,
        })
    }

    /// Builds a duration from seconds supplied as an `f64`.
    ///
    /// The fractional seconds are converted to nanoseconds without rounding to
    /// an integer nanosecond. Precision is limited by the input `f64`; use
    /// [`new`](Self::new) to keep whole seconds separate from fractional time.
    ///
    /// # Panics
    ///
    /// Panics if `secs` is negative, infinite, NaN, or at least
    /// `u32::MAX + 1` seconds.
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// assert_eq!(
    ///     Duration::from_secs_f64(1.25),
    ///     Duration::new(1, 250_000_000.0)
    /// );
    /// assert_eq!(Duration::from_secs_f64(0.5e-9), Duration::new(0, 0.5));
    /// ```
    #[must_use]
    pub fn from_secs_f64(secs: f64) -> Self {
        Self::try_from_secs_f64(secs).expect("invalid duration seconds")
    }

    /// Builds a duration from seconds supplied as an `f64`.
    ///
    /// The fallible counterpart of [`from_secs_f64`](Self::from_secs_f64).
    ///
    /// The fractional seconds are converted to nanoseconds without rounding to
    /// an integer nanosecond. Precision is limited by the input `f64`; use
    /// [`new`](Self::new) to keep whole seconds separate from fractional time.
    ///
    /// # Errors
    ///
    /// Returns [`SimError`] if `secs` is negative, infinite, NaN, or at least
    /// `u32::MAX + 1` seconds.
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// assert_eq!(
    ///     Duration::try_from_secs_f64(1.25).unwrap(),
    ///     Duration::new(1, 250_000_000.0)
    /// );
    /// assert_eq!(
    ///     Duration::try_from_secs_f64(0.5e-9).unwrap(),
    ///     Duration::new(0, 0.5)
    /// );
    /// ```
    pub fn try_from_secs_f64(secs: f64) -> Result<Self, SimError> {
        if !secs.is_finite() || !(0.0..f64::from(u32::MAX) + 1.0).contains(&secs) {
            return Err(SimError(format!("Invalid duration seconds: {secs}")));
        }
        let whole_secs = secs as u32;
        Self::try_new(
            whole_secs,
            (secs - f64::from(whole_secs)) * NANOS_PER_SEC_F64,
        )
    }

    /// Parses a decimal timestamp measured in nanoseconds, without a unit
    /// suffix.
    ///
    /// The format is ASCII digits, optionally followed by `.` and further
    /// digits. Integer nanoseconds are parsed exactly. The fractional part is
    /// rounded to `f64` precision and any carry is normalized as in
    /// [`new`](Self::new).
    ///
    /// # Errors
    ///
    /// Returns [`SimError`] for malformed text or an out-of-range duration.
    /// Signs, exponents, whitespace, NaN, and infinity are not accepted.
    ///
    /// # Examples
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::from_nanos_str("9007199254740993.125").unwrap();
    /// assert_eq!(time, Duration::new(9_007_199, 254_740_993.125));
    /// assert!(Duration::from_nanos_str("1e3").is_err());
    /// ```
    pub fn from_nanos_str(value: &str) -> Result<Self, SimError> {
        let invalid = || SimError(format!("Invalid decimal duration nanoseconds: '{value}'"));
        let (whole, fraction) = match value.split_once('.') {
            Some((whole, fraction)) => {
                if fraction.is_empty() || !fraction.bytes().all(|digit| digit.is_ascii_digit()) {
                    return Err(invalid());
                }
                (whole, Some(fraction))
            }
            None => (value, None),
        };
        if whole.is_empty() || !whole.bytes().all(|digit| digit.is_ascii_digit()) {
            return Err(invalid());
        }
        let whole = whole.parse::<u128>().map_err(|_| invalid())?;
        let duration = Self::try_from_nanos_u128(whole)?;
        match fraction {
            Some(fraction) => {
                let nanos = format!("{}.{fraction}", duration.subsec_nanos as u32)
                    .parse::<f64>()
                    .map_err(|_| invalid())?;
                Self::try_new(duration.secs, nanos)
            }
            None => Ok(duration),
        }
    }

    /// Reads the whole-second component, excluding subsecond nanoseconds.
    ///
    /// Use [`subsec_nanos`](Self::subsec_nanos) for the remaining component, or
    /// [`as_secs_f64`](Self::as_secs_f64) for an approximate total in seconds.
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// assert_eq!(Duration::new(5, 0.125).as_secs(), 5);
    /// ```
    #[must_use]
    pub const fn as_secs(&self) -> u32 {
        self.secs
    }

    /// Converts the full time span to seconds as an `f64`.
    ///
    /// This includes subsecond time, but floating-point rounding can discard
    /// small fractions when the whole-second component is large.
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// assert_eq!(Duration::new(1, 250_000_000.0).as_secs_f64(), 1.25);
    /// ```
    #[must_use]
    pub const fn as_secs_f64(&self) -> f64 {
        self.secs as f64 + self.subsec_nanos / NANOS_PER_SEC_F64
    }

    /// Reads the subsecond component in nanoseconds, including any fraction.
    ///
    /// The result is in `[0.0, 1_000_000_000.0)` and excludes whole seconds.
    /// Use [`as_nanos_f64`](Self::as_nanos_f64) for an approximate total.
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// assert_eq!(
    ///     Duration::new(5, 10_000_000.125).subsec_nanos(),
    ///     10_000_000.125
    /// );
    /// ```
    #[must_use]
    pub const fn subsec_nanos(&self) -> f64 {
        self.subsec_nanos
    }

    /// Converts the time span to whole nanoseconds, discarding any fraction.
    ///
    /// Whole nanoseconds are preserved exactly. For a fractional timestamp,
    /// use [`to_nanos_string`](Self::to_nanos_string), or
    /// [`as_nanos_f64`](Self::as_nanos_f64) if floating-point rounding is
    /// acceptable.
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// assert_eq!(Duration::new(1, 0.75).as_nanos(), 1_000_000_000);
    /// ```
    #[must_use]
    pub const fn as_nanos(&self) -> u128 {
        self.secs as u128 * NANOS_PER_SEC as u128 + self.subsec_nanos as u128
    }

    /// Converts the full time span to nanoseconds as an `f64`.
    ///
    /// This includes fractional nanoseconds, but can lose integer and
    /// fractional precision for large timestamps.
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// assert_eq!(Duration::new(1, 0.125).as_nanos_f64(), 1_000_000_000.125);
    /// ```
    #[must_use]
    pub const fn as_nanos_f64(&self) -> f64 {
        self.secs as f64 * NANOS_PER_SEC_F64 + self.subsec_nanos
    }

    /// Formats a decimal nanosecond timestamp without a unit suffix.
    ///
    /// The output preserves every integer nanosecond and enough fractional
    /// digits to recover the stored value with
    /// [`from_nanos_str`](Self::from_nanos_str). Whole nanoseconds have no
    /// decimal point. Formatting never converts the entire timestamp to `f64`.
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::new(9_007_199, 254_740_993.125);
    /// let text = time.to_nanos_string();
    /// assert_eq!(text, "9007199254740993.125");
    /// assert_eq!(Duration::from_nanos_str(&text).unwrap(), time);
    /// ```
    #[must_use]
    pub fn to_nanos_string(&self) -> String {
        let whole = self.as_nanos();
        let nanos = self.subsec_nanos.to_string();
        match nanos.split_once('.') {
            Some((_, fraction)) => format!("{whole}.{fraction}"),
            None => whole.to_string(),
        }
    }

    /// Adds two time spans, or returns [`None`] when their sum exceeds
    /// [`MAX`](Self::MAX).
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let half = Duration::new(0, 0.5);
    /// assert_eq!(half.checked_add(half), Some(Duration::from_nanos(1)));
    /// assert_eq!(Duration::MAX.checked_add(half), None);
    /// ```
    #[must_use]
    pub const fn checked_add(self, rhs: Self) -> Option<Self> {
        let mut secs = match self.secs.checked_add(rhs.secs) {
            Some(secs) => secs,
            None => return None,
        };
        let mut nanos = self.subsec_nanos + rhs.subsec_nanos;
        if nanos >= NANOS_PER_SEC_F64 {
            secs = match secs.checked_add(1) {
                Some(secs) => secs,
                None => return None,
            };
            nanos -= NANOS_PER_SEC_F64;
        }
        Some(Self {
            secs,
            subsec_nanos: nanos,
        })
    }

    /// Adds two time spans, clamping an overflowing sum to [`MAX`](Self::MAX).
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let half = Duration::new(0, 0.5);
    /// assert_eq!(half.saturating_add(half), Duration::from_nanos(1));
    /// assert_eq!(Duration::MAX.saturating_add(half), Duration::MAX);
    /// ```
    #[must_use]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        match self.checked_add(rhs) {
            Some(duration) => duration,
            None => Self::MAX,
        }
    }

    /// Subtracts `rhs`, or returns [`None`] when the result would be negative.
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let half = Duration::new(0, 0.5);
    /// assert_eq!(half.checked_sub(half), Some(Duration::ZERO));
    /// assert_eq!(Duration::ZERO.checked_sub(half), None);
    /// ```
    #[must_use]
    pub const fn checked_sub(self, rhs: Self) -> Option<Self> {
        let mut secs = match self.secs.checked_sub(rhs.secs) {
            Some(secs) => secs,
            None => return None,
        };
        let mut nanos = self.subsec_nanos - rhs.subsec_nanos;
        if nanos < 0.0 {
            secs = match secs.checked_sub(1) {
                Some(secs) => secs,
                None => return None,
            };
            nanos += NANOS_PER_SEC_F64;
        }
        if nanos == NANOS_PER_SEC_F64 {
            secs += 1;
            nanos = 0.0;
        }
        Some(Self {
            secs,
            subsec_nanos: nanos,
        })
    }

    /// Subtracts `rhs`, clamping a negative result to [`ZERO`](Self::ZERO).
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let half = Duration::new(0, 0.5);
    /// assert_eq!(half.saturating_sub(half), Duration::ZERO);
    /// assert_eq!(Duration::ZERO.saturating_sub(half), Duration::ZERO);
    /// ```
    #[must_use]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        match self.checked_sub(rhs) {
            Some(duration) => duration,
            None => Self::ZERO,
        }
    }

    /// Scales a time span by `rhs`, or returns [`None`] if it exceeds
    /// [`MAX`](Self::MAX).
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::new(0, 500_000_000.25);
    /// assert_eq!(time.checked_mul(2), Some(Duration::new(1, 0.5)));
    /// assert_eq!(Duration::MAX.checked_mul(2), None);
    /// ```
    #[must_use]
    pub fn checked_mul(self, rhs: u32) -> Option<Self> {
        let whole = Self::try_from_nanos_u128(self.as_nanos() * u128::from(rhs)).ok()?;
        let fraction = Self::normalize(0, self.subsec_nanos.fract() * f64::from(rhs))?;
        whole.checked_add(fraction)
    }

    /// Scales a time span by `rhs`, clamping overflow to [`MAX`](Self::MAX).
    ///
    /// ```
    /// use gwr_std::time::Duration;
    ///
    /// let time = Duration::new(0, 500_000_000.25);
    /// assert_eq!(time.saturating_mul(2), Duration::new(1, 0.5));
    /// assert_eq!(Duration::MAX.saturating_mul(2), Duration::MAX);
    /// ```
    #[must_use]
    pub fn saturating_mul(self, rhs: u32) -> Self {
        match self.checked_mul(rhs) {
            Some(duration) => duration,
            None => Self::MAX,
        }
    }

    /// Writes a human-readable time span to a formatter, as used by
    /// [`Display`].
    ///
    /// Selects the first of seconds, milliseconds, or microseconds whose value
    /// exceeds one, falling back to nanoseconds. Unit suffixes are `s`, `ms`,
    /// `us`, and `ns`. Seconds, milliseconds, and microseconds use two decimal
    /// places by default, or the formatter's requested precision. Nanoseconds
    /// use the stored `f64`'s display format and ignore the requested
    /// precision.
    ///
    /// # Errors
    ///
    /// Returns a formatting error if writing to the formatter fails.
    pub fn to_appropriate_unit_fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let precision = f.precision().unwrap_or(2);
        let time_s = self.as_secs_f64();
        if time_s > 1.0 {
            write!(f, "{time_s:.precision$}s")
        } else if time_s > 1.0e-3 {
            write!(f, "{:.precision$}ms", time_s * 1.0e3)
        } else if time_s > 1.0e-6 {
            write!(f, "{:.precision$}us", time_s * 1.0e6)
        } else {
            write!(f, "{}ns", self.subsec_nanos)
        }
    }

    fn normalize(secs: u32, nanos: f64) -> Option<Self> {
        if !nanos.is_finite()
            || nanos < 0.0
            || nanos >= (f64::from(u32::MAX) + 1.0) * NANOS_PER_SEC_F64
        {
            return None;
        }
        let whole_nanos = nanos as u64;
        let carry = u32::try_from(whole_nanos / NANOS_PER_SEC).ok()?;
        let secs = secs.checked_add(carry)?;
        let subsec_nanos = (whole_nanos % NANOS_PER_SEC) as f64 + (nanos - whole_nanos as f64);
        Some(Self {
            secs,
            subsec_nanos: if subsec_nanos == 0.0 {
                0.0
            } else {
                subsec_nanos
            },
        })
    }
}

/// Formats a time span using a suitable unit and an ASCII unit suffix.
///
/// For a timestamp that preserves the stored value, use
/// [`Duration::to_nanos_string`] instead.
///
/// ```
/// use gwr_std::time::Duration;
///
/// assert_eq!(Duration::from_secs(1).to_string(), "1000.00ms");
/// assert_eq!(format!("{:.3}", Duration::from_millis(125)), "125.000ms");
/// assert_eq!(format!("{:.1}", Duration::new(0, 0.125)), "0.125ns");
/// ```
impl Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.to_appropriate_unit_fmt(f)
    }
}

impl Add for Duration {
    type Output = Self;

    /// Adds two durations, preserving fractional nanoseconds.
    ///
    /// # Panics
    ///
    /// Panics if [`Duration::checked_add`] returns [`None`].
    fn add(self, rhs: Self) -> Self {
        self.checked_add(rhs).expect("Duration addition overflow")
    }
}

impl Sub for Duration {
    type Output = Self;

    /// Subtracts one duration from another, preserving fractional nanoseconds.
    ///
    /// # Panics
    ///
    /// Panics if [`Duration::checked_sub`] returns [`None`].
    fn sub(self, rhs: Self) -> Self {
        self.checked_sub(rhs)
            .expect("Duration subtraction underflow")
    }
}

impl Mul<u32> for Duration {
    type Output = Self;

    /// Scales a duration by an integer factor, preserving fractional
    /// nanoseconds.
    ///
    /// # Panics
    ///
    /// Panics if [`Duration::checked_mul`] returns [`None`].
    fn mul(self, rhs: u32) -> Self {
        self.checked_mul(rhs)
            .expect("Duration multiplication overflow")
    }
}
