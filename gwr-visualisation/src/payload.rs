// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#[cfg(all(test, feature = "generator"))]
use std::io::Read;
#[cfg(feature = "generator")]
use std::io::Write;

#[cfg(feature = "generator")]
use flate2::Compression;
#[cfg(all(test, feature = "generator"))]
use flate2::read::GzDecoder;
#[cfg(feature = "generator")]
use flate2::write::GzEncoder;
#[cfg(feature = "generator")]
use serde::Serialize;
#[cfg(all(test, feature = "generator"))]
use serde::de::DeserializeOwned;

#[cfg(feature = "generator")]
pub(crate) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let json = serde_json::to_vec(value)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&json)?;
    Ok(encoder.finish()?)
}

#[cfg(all(test, feature = "generator"))]
pub(crate) fn decode<T: DeserializeOwned>(compressed: &[u8]) -> Result<T, String> {
    let mut decoder = GzDecoder::new(compressed);
    let mut json = Vec::new();
    decoder
        .read_to_end(&mut json)
        .map_err(|error| format!("Unable to decompress report data: {error}"))?;
    serde_json::from_slice(&json).map_err(|error| format!("Unable to parse report data: {error}"))
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[cfg(feature = "generator")]
    use super::{decode, encode};

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct Example {
        name: String,
        values: Vec<u64>,
    }

    #[test]
    #[cfg(feature = "generator")]
    fn round_trips_typed_data() {
        let expected = Example {
            name: "large timetable".to_string(),
            values: vec![1, 2, 3, u64::MAX],
        };

        let compressed = encode(&expected).unwrap();

        assert_eq!(decode::<Example>(&compressed).unwrap(), expected);
    }

    #[test]
    #[cfg(feature = "generator")]
    fn rejects_invalid_compressed_data() {
        let error = decode::<Example>(b"not gzip").unwrap_err();

        assert!(error.contains("decompress"));
    }
}
