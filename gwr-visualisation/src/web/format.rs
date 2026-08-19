// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

pub(crate) fn escape(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(crate) fn integer(value: impl Into<u128>) -> String {
    let digits = value.into().to_string();
    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(character);
    }
    result
}

pub(crate) fn count(value: f64) -> String {
    if value.abs() < 10_000.0 {
        return integer(value.round().max(0.0) as u128);
    }
    for (scale, suffix) in [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K")] {
        if value.abs() >= scale {
            return trim_decimal(value / scale, 2) + suffix;
        }
    }
    integer(value.round().max(0.0) as u128)
}

pub(crate) fn count_u64(value: u64) -> String {
    if value < 10_000 {
        return integer(value);
    }
    for (scale, suffix) in [
        (1_000_000_000_000_u64, "T"),
        (1_000_000_000, "B"),
        (1_000_000, "M"),
        (1_000, "K"),
    ] {
        if value >= scale {
            return format_hundredths(value, scale) + suffix;
        }
    }
    integer(value)
}

pub(crate) fn bytes(value: f64) -> String {
    let mut scaled = value;
    let mut unit = "B";
    for candidate in ["KiB", "MiB", "GiB", "TiB"] {
        if scaled < 1024.0 {
            break;
        }
        scaled /= 1024.0;
        unit = candidate;
    }
    let precision = usize::from(scaled < 10.0 && unit != "B");
    format!("{} {unit}", trim_decimal(scaled, precision))
}

pub(crate) fn bytes_u64(value: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB"];
    let mut divisor = 1_u64;
    let mut unit = 0;
    while unit < units.len() - 1 && value >= divisor.saturating_mul(1024) {
        divisor = divisor.saturating_mul(1024);
        unit += 1;
    }
    if unit == 0 {
        return format!("{} B", integer(value));
    }
    let whole = value / divisor;
    if whole >= 10 {
        let rounded = (u128::from(value) + u128::from(divisor / 2)) / u128::from(divisor);
        return format!("{} {}", integer(rounded), units[unit]);
    }
    let tenths = (u128::from(value) * 10 + u128::from(divisor / 2)) / u128::from(divisor);
    format!("{}.{} {}", integer(tenths / 10), tenths % 10, units[unit])
}

fn format_hundredths(value: u64, divisor: u64) -> String {
    let scaled = (u128::from(value) * 100 + u128::from(divisor / 2)) / u128::from(divisor);
    let whole = scaled / 100;
    let fraction = scaled % 100;
    if fraction == 0 {
        integer(whole)
    } else if fraction.is_multiple_of(10) {
        format!("{}.{:01}", integer(whole), fraction / 10)
    } else {
        format!("{}.{fraction:02}", integer(whole))
    }
}

pub(crate) fn hex(value: u64) -> String {
    format!("0x{value:x}")
}

pub(crate) fn label(name: &str) -> String {
    let words = name.replace('_', " ");
    let mut characters = words.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

pub(crate) fn decimal(value: f64, precision: usize) -> String {
    trim_decimal(value, precision)
}

fn trim_decimal(value: f64, precision: usize) -> String {
    let formatted = format!("{value:.precision$}");
    if precision == 0 {
        return formatted;
    }
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{bytes, bytes_u64, count, count_u64, escape, integer};

    #[test]
    fn formats_report_values() {
        assert_eq!(integer(1_234_567_u64), "1,234,567");
        assert_eq!(count(1_250_000.0), "1.25M");
        assert_eq!(count_u64(1_250_000), "1.25M");
        assert_eq!(bytes(1_536.0), "1.5 KiB");
        assert_eq!(bytes_u64(1_536), "1.5 KiB");
        assert_eq!(bytes_u64(u64::MAX), "17,179,869,184 GiB");
        assert_eq!(
            escape("<tensor & \"name\">"),
            "&lt;tensor &amp; &quot;name&quot;&gt;"
        );
    }
}
