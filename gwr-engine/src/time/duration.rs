// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! This module provides a mechanism for [Display]ing a [Duration].

use std::fmt::Display;
use std::time::Duration;

/// The [AppropriateUnitDisplay] trait provides a mechanism for formatting a
/// type to an appropriate unit.
pub trait AppropriateUnitDisplay {
    fn to_appropriate_unit_fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result;
}

/// Implement [AppropriateUnitDisplay] for [Duration] to allow it to be
/// formatted to an appropriate unit of time. The unit, chosen based on the
/// value, will be either seconds (s), milliseconds (ms), microseconds (us), or
/// nanoseconds (ns).
///
/// As [Duration] cannot represent sub-nanosecond times any value formatted to
/// ns will be done so as an integer.
impl AppropriateUnitDisplay for Duration {
    fn to_appropriate_unit_fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let precision = f.precision().unwrap_or(2);
        let time_s = self.as_secs_f64();
        if time_s > 1.0 {
            write!(f, "{time_s:.precision$}s")
        } else if time_s > 1.0e-3 {
            write!(f, "{:.precision$}ms", time_s * 1.0e3)
        } else if time_s > 1.0e-6 {
            write!(f, "{:.precision$}us", time_s * 1.0e6)
        } else {
            write!(f, "{}ns", time_s * 1.0e9)
        }
    }
}

/// Implement [Display] for [AppropriateUnitDisplay] to allow [Duration],
/// which itself does not implement [Display], to be formatted using the
/// [AppropriateUnitDisplay] implementation provided by this module.
///
/// This enables [Duration]s to be [Display]ed as follows:
/// ```rust
/// use std::time::Duration;
///
/// use gwr_engine::time::duration::AppropriateUnitDisplay;
///
/// println!("{}", &Duration::new(1, 0) as &dyn AppropriateUnitDisplay);
/// ```
impl Display for dyn AppropriateUnitDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.to_appropriate_unit_fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn fmt(d: Duration) -> String {
        format!("{}", &d as &dyn AppropriateUnitDisplay)
    }

    fn fmt_precision(d: Duration, precision: usize) -> String {
        format!("{:.precision$}", &d as &dyn AppropriateUnitDisplay)
    }

    #[test]
    fn seconds() {
        assert_eq!(fmt(Duration::from_secs(5)), "5.00s");
        assert_eq!(fmt(Duration::from_secs_f64(1.5)), "1.50s");
        assert_eq!(fmt(Duration::from_secs(100)), "100.00s");
        assert_eq!(fmt(Duration::from_nanos(2_000_567_890)), "2.00s");
    }

    #[test]
    fn milliseconds() {
        assert_eq!(fmt(Duration::from_millis(500)), "500.00ms");
        assert_eq!(fmt(Duration::from_millis(2)), "2.00ms");
        assert_eq!(fmt(Duration::from_secs_f64(0.1)), "100.00ms");
    }

    #[test]
    fn microseconds() {
        assert_eq!(fmt(Duration::from_micros(500)), "500.00us");
        assert_eq!(fmt(Duration::from_micros(2)), "2.00us");
        assert_eq!(fmt(Duration::from_secs_f64(0.0001)), "100.00us");
    }

    #[test]
    fn nanoseconds() {
        assert_eq!(fmt(Duration::from_nanos(500)), "500ns");
        assert_eq!(fmt(Duration::from_nanos(1)), "1ns");
        assert_eq!(fmt(Duration::ZERO), "0ns");
    }

    #[test]
    fn duration_precision() {
        assert_eq!(fmt(Duration::from_nanos(2_009_000_000)), "2.01s");
    }

    #[test]
    fn custom_precision() {
        assert_eq!(fmt_precision(Duration::from_secs(5), 0), "5s");
        assert_eq!(fmt_precision(Duration::from_millis(500), 4), "500.0000ms");
        assert_eq!(fmt_precision(Duration::from_micros(3), 1), "3.0us");
        assert_eq!(fmt_precision(Duration::from_nanos(42), 3), "42ns");
    }

    #[test]
    fn boundary_values() {
        // Exactly at 1s boundary — should format as ms (not > 1.0)
        assert_eq!(fmt(Duration::from_secs(1)), "1000.00ms");
        // Exactly at 1ms boundary — should format as us
        assert_eq!(fmt(Duration::from_millis(1)), "1000.00us");
        // Exactly at 1us boundary — should format as ns
        assert_eq!(fmt(Duration::from_micros(1)), "1000ns");
    }
}
