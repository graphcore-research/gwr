// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use byte_unit::Byte;

/// Parse a CLI byte value, accepting either a bare byte count or a
/// `byte-unit` string such as `32KiB`, `1484B`, or `2MiB`.
pub fn parse_bytes_string(value: &str) -> Result<usize, String> {
    // Don't ignore case so that bit (b) and Byte (B) can be distinguished.
    let ignore_case = false;
    let bytes = Byte::parse_str(value, ignore_case)
        .map_err(|e| format!("Unable to parse {value} as Byte string: {e}"))?
        .as_u64();
    usize::try_from(bytes).map_err(|e| format!("{value} is too large for this platform: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_buffer_bytes_accepts_binary_units() {
        assert_eq!(parse_bytes_string("32KiB").unwrap(), 32 * 1024);
    }

    #[test]
    fn parse_buffer_bytes_accepts_byte_units() {
        assert_eq!(parse_bytes_string("32768B").unwrap(), 32 * 1024);
    }

    #[test]
    fn parse_buffer_bytes_accepts_bare_byte_counts() {
        assert_eq!(parse_bytes_string("32768").unwrap(), 32 * 1024);
    }

    #[test]
    fn parse_buffer_bytes_rejects_invalid_values() {
        assert!(parse_bytes_string("thirty-two").is_err());
    }
}
