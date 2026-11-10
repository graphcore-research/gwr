// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use byte_unit::Byte;
use serde::{Deserialize, Deserializer};

/// Deserialize a byte count from either an integer or a byte-unit
/// string.
pub fn deserialize_bytes<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ByteValue {
        Integer(u64),
        String(String),
    }

    match ByteValue::deserialize(deserializer)? {
        ByteValue::Integer(value) => usize::try_from(value).map_err(|error| error.to_string()),
        ByteValue::String(value) => parse_bytes_string(&value),
    }
    .map_err(serde::de::Error::custom)
}

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

    #[derive(Deserialize)]
    struct ByteConfig {
        #[serde(deserialize_with = "deserialize_bytes")]
        bytes: usize,
    }

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

    #[test]
    fn deserialize_bytes_accepts_unit_strings() {
        let config: ByteConfig = serde_json::from_str(r#"{"bytes":"1MiB"}"#).unwrap();

        assert_eq!(config.bytes, 1_048_576);
    }

    #[test]
    fn deserialize_bytes_accepts_integers() {
        let config: ByteConfig = serde_json::from_str(r#"{"bytes":1024}"#).unwrap();

        assert_eq!(config.bytes, 1024);
    }
}
