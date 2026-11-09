// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Opt-in numerical payload storage for simulated program execution.
//!
//! Enable the crate's `functional` feature, load physical byte addresses, then
//! attach the same store to every participating processing element. Ordinary
//! builds contain neither this storage nor transaction payload fields.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use gwr_engine::types::{AccessType, SimError, SimResult};

use super::FunctionalValue;
use crate::processing_element::tensors::dtype::DataType;

/// Sparse physical storage with per-bit initialization. Missing values are
/// errors; unwritten packed neighbors are never manufactured as zeros.
#[derive(Debug, Default)]
pub struct FunctionalMemory {
    pages: RefCell<BTreeMap<u64, Page>>,
}

// Pages keep large tensor payloads compact while retaining byte-exact
// missing-data detection, unaligned transfers and masked writes. Sparse
// partial masks retain individual bits only at partially initialized bytes.
const PAGE_BYTES: usize = 65536;
#[derive(Debug)]
struct Page {
    bytes: Box<[u8]>,
    initialized: Box<[u64]>,
    partial: BTreeMap<usize, u8>,
}
impl Default for Page {
    fn default() -> Self {
        Self {
            bytes: vec![0; PAGE_BYTES].into_boxed_slice(),
            initialized: vec![0; PAGE_BYTES / 64].into_boxed_slice(),
            partial: BTreeMap::new(),
        }
    }
}

/// Numerical comparison evidence for a checked output range.
#[derive(Clone, Debug, Default)]
pub struct FunctionalCheck {
    pub elements: usize,
    pub max_absolute_error: f64,
    pub max_relative_error: f64,
}

impl FunctionalMemory {
    /// Load typed values at a byte address. Packed types use least-significant
    /// bits first, without padding between logical elements.
    pub fn write_values(
        &self,
        address: u64,
        dtype: DataType,
        values: &[FunctionalValue],
    ) -> SimResult {
        let count = values
            .len()
            .checked_mul(dtype.num_bits())
            .ok_or_else(|| SimError("functional bit count overflows".into()))?;
        check_range(address, count.div_ceil(8))?;
        if values.iter().any(|value| value.dtype() != dtype) {
            return Err(SimError("functional load dtype mismatch".into()));
        }
        for (index, value) in values.iter().enumerate() {
            self.write_value(
                address as u128 * 8 + (index * dtype.num_bits()) as u128,
                *value,
            )?;
        }
        Ok(())
    }

    /// Read typed values. Only the requested logical bits must be initialized.
    pub fn read_values(
        &self,
        address: u64,
        dtype: DataType,
        count: usize,
    ) -> Result<Vec<FunctionalValue>, SimError> {
        let bits = count
            .checked_mul(dtype.num_bits())
            .ok_or_else(|| SimError("functional bit count overflows".into()))?;
        check_range(address, bits.div_ceil(8))?;
        (0..count)
            .map(|index| {
                self.read_value(
                    address as u128 * 8 + (index * dtype.num_bits()) as u128,
                    dtype,
                )
            })
            .collect()
    }

    /// Compare exact typed encodings, including full-width integer values.
    pub fn check_values(
        &self,
        address: u64,
        dtype: DataType,
        expected: &[FunctionalValue],
    ) -> SimResult {
        let actual = self.read_values(address, dtype, expected.len())?;
        for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            if actual != expected {
                return Err(SimError(format!(
                    "functional typed mismatch at element {index}: actual {actual:?}, expected {expected:?}"
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn read_value(
        &self,
        bit_address: u128,
        dtype: DataType,
    ) -> Result<FunctionalValue, SimError> {
        let (address, offset, count) = bit_range(bit_address, dtype.num_bits())?;
        if offset == 0 && dtype.num_bits().is_multiple_of(8) {
            let bytes = self.read_bytes(address, count)?;
            let mut word = [0; 8];
            word[..count].copy_from_slice(&bytes);
            return FunctionalValue::from_bits(dtype, u64::from_le_bytes(word));
        }
        let masks = bit_masks(offset, dtype.num_bits(), count);
        let bytes = self.read_masked(address, &masks)?;
        let mut bits = 0u128;
        for (i, byte) in bytes.iter().enumerate() {
            bits |= (*byte as u128) << (i * 8);
        }
        FunctionalValue::from_bits(
            dtype,
            ((bits >> offset) as u64) & super::functional_value::mask(dtype),
        )
    }

    pub(crate) fn write_value(&self, bit_address: u128, value: FunctionalValue) -> SimResult {
        let (address, offset, count) = bit_range(bit_address, value.dtype().num_bits())?;
        if offset == 0 && value.dtype().num_bits().is_multiple_of(8) {
            return self.write_bytes(address, &value.bits().to_le_bytes()[..count]);
        }
        let bits = (value.bits() as u128) << offset;
        let bytes = (0..count)
            .map(|i| (bits >> (i * 8)) as u8)
            .collect::<Vec<_>>();
        self.write_masked(
            address,
            &bytes,
            &bit_masks(offset, value.dtype().num_bits(), count),
        )
    }

    fn read_masked(&self, address: u64, masks: &[u8]) -> Result<Vec<u8>, SimError> {
        check_range(address, masks.len())?;
        let pages = self.pages.borrow();
        masks
            .iter()
            .enumerate()
            .map(|(i, mask)| {
                let current = address + i as u64;
                let page = pages.get(&(current / PAGE_BYTES as u64)).ok_or_else(|| {
                    SimError(format!("uninitialized functional byte at {current:#x}"))
                })?;
                let offset = current as usize % PAGE_BYTES;
                let initialized = if page.initialized[offset / 64] & (1 << (offset % 64)) != 0 {
                    255
                } else {
                    page.partial.get(&offset).copied().unwrap_or(0)
                };
                if mask & !initialized != 0 {
                    return Err(SimError(format!(
                        "uninitialized functional bits at {current:#x}"
                    )));
                }
                Ok(page.bytes[offset] & mask)
            })
            .collect()
    }

    fn write_masked(&self, address: u64, bytes: &[u8], masks: &[u8]) -> SimResult {
        check_range(address, bytes.len())?;
        let mut pages = self.pages.borrow_mut();
        for (i, (&byte, &mask)) in bytes.iter().zip(masks).enumerate() {
            if mask == 0 {
                continue;
            }
            let current = address + i as u64;
            let offset = current as usize % PAGE_BYTES;
            let page = pages.entry(current / PAGE_BYTES as u64).or_default();
            page.bytes[offset] = (page.bytes[offset] & !mask) | (byte & mask);
            if page.initialized[offset / 64] & (1 << (offset % 64)) == 0 {
                let initialized = page.partial.entry(offset).or_default();
                *initialized |= mask;
                if *initialized == 255 {
                    page.initialized[offset / 64] |= 1 << (offset % 64);
                    page.partial.remove(&offset);
                }
            }
        }
        Ok(())
    }

    /// Load little-endian FP32 values at a physical byte address.
    pub fn write_f32(&self, address: u64, values: &[f32]) -> SimResult {
        self.write_bytes(
            address,
            &values
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        )
    }

    /// Read initialized little-endian FP32 values, including aliased ranges.
    pub fn read_f32(&self, address: u64, count: usize) -> Result<Vec<f32>, SimError> {
        let bytes = count
            .checked_mul(4)
            .ok_or_else(|| SimError("functional byte count overflows".into()))?;
        Ok(self
            .read_bytes(address, bytes)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes))
            .collect())
    }

    /// Compare against independent expected values using `atol +
    /// rtol*abs(expected)`. Nonfinite values and tolerances fail, including
    /// matching NaNs/infinities.
    pub fn check_f32(
        &self,
        address: u64,
        expected: &[f32],
        atol: f32,
        rtol: f32,
    ) -> Result<FunctionalCheck, SimError> {
        if !atol.is_finite() || !rtol.is_finite() || atol < 0.0 || rtol < 0.0 {
            return Err(SimError(
                "functional tolerances must be finite and nonnegative".into(),
            ));
        }
        let actual = self.read_f32(address, expected.len())?;
        let mut check = FunctionalCheck {
            elements: expected.len(),
            ..Default::default()
        };
        for (index, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
            let error = (f64::from(actual) - f64::from(expected)).abs();
            if !actual.is_finite()
                || !expected.is_finite()
                || error > f64::from(atol) + f64::from(rtol) * f64::from(expected).abs()
            {
                return Err(SimError(format!(
                    "functional mismatch at element {index} (address {address:#x}): actual {actual}, expected {expected}, absolute error {error}"
                )));
            }
            check.max_absolute_error = check.max_absolute_error.max(error);
            check.max_relative_error = check.max_relative_error.max(if expected == 0.0 {
                if error == 0.0 { 0.0 } else { f64::INFINITY }
            } else {
                error / f64::from(expected).abs()
            });
        }
        Ok(check)
    }

    /// Snapshot received bits without manufacturing initialization for packet
    /// boundary padding. The evaluator still requires every consumed bit.
    fn read_snapshot(
        &self,
        address: u64,
        count: usize,
    ) -> Result<(Vec<u8>, Option<Vec<u8>>), SimError> {
        if let Ok(bytes) = self.read_bytes(address, count) {
            return Ok((bytes, None));
        }
        check_range(address, count)?;
        let pages = self.pages.borrow();
        let mut bytes = Vec::with_capacity(count);
        let mut masks = Vec::with_capacity(count);
        for i in 0..count {
            let current = address + i as u64;
            let offset = current as usize % PAGE_BYTES;
            let (byte, mask) = pages
                .get(&(current / PAGE_BYTES as u64))
                .map_or((0, 0), |page| {
                    (
                        page.bytes[offset],
                        if page.initialized[offset / 64] & (1 << (offset % 64)) != 0 {
                            255
                        } else {
                            page.partial.get(&offset).copied().unwrap_or(0)
                        },
                    )
                });
            if mask == 0 {
                return Err(SimError(format!(
                    "uninitialized functional byte at {current:#x}"
                )));
            }
            bytes.push(byte);
            masks.push(mask);
        }
        Ok((bytes, Some(masks)))
    }

    pub(crate) fn read_bytes(&self, address: u64, count: usize) -> Result<Vec<u8>, SimError> {
        check_range(address, count)?;
        let pages = self.pages.borrow();
        let mut result = Vec::with_capacity(count);
        let mut current = address;
        while result.len() < count {
            let offset = current as usize % PAGE_BYTES;
            let length = (PAGE_BYTES - offset).min(count - result.len());
            let page = pages.get(&(current / PAGE_BYTES as u64)).ok_or_else(|| {
                SimError(format!("uninitialized functional byte at {current:#x}"))
            })?;
            for (word, mask) in initialization_masks(offset, length) {
                let missing = mask & !page.initialized[word];
                if missing != 0 {
                    return Err(SimError(format!(
                        "uninitialized functional byte at {:#x}",
                        current - offset as u64
                            + (word * 64 + missing.trailing_zeros() as usize) as u64
                    )));
                }
            }
            result.extend_from_slice(&page.bytes[offset..offset + length]);
            current = current.wrapping_add(length as u64);
        }
        Ok(result)
    }

    pub(crate) fn write_bytes(&self, address: u64, values: &[u8]) -> SimResult {
        check_range(address, values.len())?;
        let mut pages = self.pages.borrow_mut();
        let mut written = 0;
        while written < values.len() {
            let current = address + written as u64;
            let offset = current as usize % PAGE_BYTES;
            let length = (PAGE_BYTES - offset).min(values.len() - written);
            let page = pages.entry(current / PAGE_BYTES as u64).or_default();
            page.bytes[offset..offset + length].copy_from_slice(&values[written..written + length]);
            for (word, mask) in initialization_masks(offset, length) {
                page.initialized[word] |= mask;
            }
            if !page.partial.is_empty() {
                page.partial
                    .retain(|key, _| *key < offset || *key >= offset + length);
            }
            written += length;
        }
        Ok(())
    }
}

fn bit_range(address: u128, bits: usize) -> Result<(u64, usize, usize), SimError> {
    let byte = u64::try_from(address / 8)
        .map_err(|_| SimError("functional bit address overflows".into()))?;
    let offset = (address % 8) as usize;
    let count = (offset + bits).div_ceil(8);
    check_range(byte, count)?;
    Ok((byte, offset, count))
}

fn bit_masks(offset: usize, bits: usize, count: usize) -> Vec<u8> {
    let mask = ((1u128 << bits) - 1) << offset;
    (0..count).map(|i| (mask >> (i * 8)) as u8).collect()
}

/// Masks cover only the requested bytes, including partial boundary words.
fn initialization_masks(offset: usize, length: usize) -> impl Iterator<Item = (usize, u64)> {
    let end = offset + length;
    (offset / 64..end.div_ceil(64)).map(move |word| {
        let low = offset.saturating_sub(word * 64);
        let high = (end - word * 64).min(64);
        (word, (u64::MAX << low) & (u64::MAX >> (64 - high)))
    })
}

fn check_range(address: u64, count: usize) -> SimResult {
    if count != 0
        && address
            .checked_add(u64::try_from(count - 1).map_err(|e| SimError(e.to_string()))?)
            .is_none()
    {
        return Err(SimError("functional address range overflows".into()));
    }
    Ok(())
}

type FunctionalValues = BTreeMap<(String, Vec<usize>), FunctionalValue>;

/// Per-tile received bytes and evaluated intermediate coordinates.
#[derive(Clone, Debug)]
pub(crate) struct FunctionalContext {
    pub backing: Rc<FunctionalMemory>,
    pub local: Rc<FunctionalMemory>,
    pub values: Rc<RefCell<FunctionalValues>>,
    writes: Rc<RefCell<BTreeMap<u64, u8>>>,
}

impl FunctionalContext {
    pub fn new(backing: Rc<FunctionalMemory>) -> Self {
        Self {
            backing,
            local: Rc::default(),
            values: Rc::default(),
            writes: Rc::default(),
        }
    }

    pub fn write_value(&self, address: u128, value: FunctionalValue) -> SimResult {
        self.local.write_value(address, value)?;
        let (byte, offset, count) = bit_range(address, value.dtype().num_bits())?;
        if value.dtype().num_bits() < 8 {
            let mut writes = self.writes.borrow_mut();
            for (i, mask) in bit_masks(offset, value.dtype().num_bits(), count)
                .into_iter()
                .enumerate()
            {
                *writes.entry(byte + i as u64).or_default() |= mask;
            }
        }
        Ok(())
    }

    pub fn copy_bytes(&self, source: u64, destination: u64, count: usize) -> SimResult {
        let (bytes, masks) = self.local.read_snapshot(source, count)?;
        if let Some(masks) = masks {
            self.local.write_masked(destination, &bytes, &masks)?;
            let mut writes = self.writes.borrow_mut();
            for (i, mask) in masks.into_iter().enumerate() {
                if mask != 255 {
                    *writes.entry(destination + i as u64).or_default() |= mask;
                }
            }
        } else {
            self.local.write_bytes(destination, &bytes)?;
        }
        Ok(())
    }

    pub fn fresh(&self) -> Self {
        Self::new(self.backing.clone())
    }

    pub fn packet(
        &self,
        kind: AccessType,
        address: u64,
        count: usize,
    ) -> Result<FunctionalPacket, SimError> {
        // Ordinary byte ranges cover only this output. Boundary validation
        // forbids overlapping output allocations; packed boundary bytes use
        // explicit dirty masks, independent of any initialized input bits.
        let masks = if kind == AccessType::WriteNonPostedRequest {
            let writes = self.writes.borrow();
            if writes
                .range(
                    address
                        ..=address
                            .checked_add(count.saturating_sub(1) as u64)
                            .ok_or_else(|| SimError("functional packet range overflows".into()))?,
                )
                .next()
                .is_some()
            {
                Some(
                    (0..count)
                        .map(|i| writes.get(&(address + i as u64)).copied().unwrap_or(255))
                        .collect::<Vec<_>>(),
                )
            } else {
                None
            }
        } else {
            None
        };
        let payload = match kind {
            AccessType::ReadRequest => None,
            AccessType::WriteNonPostedRequest => Some(if let Some(masks) = &masks {
                self.local.read_masked(address, masks)?
            } else {
                self.local.read_bytes(address, count)?
            }),
            _ => {
                return Err(SimError(
                    "functional packets require reads or nonposted writes".into(),
                ));
            }
        };
        Ok(FunctionalPacket {
            context: self.clone(),
            address,
            count,
            payload,
            masks,
        })
    }
}

/// Logical bit mask travels with the actual aligned packet. Padding is never
/// read or overwritten, so neighboring tiles may share one physical packet.
#[derive(Clone, Debug)]
pub(crate) struct FunctionalPacket {
    context: FunctionalContext,
    address: u64,
    count: usize,
    payload: Option<Vec<u8>>,
    masks: Option<Vec<u8>>,
}

impl FunctionalPacket {
    pub fn respond(&self, kind: AccessType) -> Result<Self, SimError> {
        let mut response = self.clone();
        match kind {
            AccessType::ReadRequest => {
                let (bytes, masks) = self
                    .context
                    .backing
                    .read_snapshot(self.address, self.count)?;
                response.payload = Some(bytes);
                response.masks = masks;
            }
            AccessType::WriteNonPostedRequest => {
                let bytes = self
                    .payload
                    .as_ref()
                    .ok_or_else(|| SimError("functional write has no payload".into()))?;
                if let Some(masks) = &self.masks {
                    self.context
                        .backing
                        .write_masked(self.address, bytes, masks)?;
                } else {
                    self.context.backing.write_bytes(self.address, bytes)?;
                }
            }
            _ => return Err(SimError("unsupported functional packet response".into())),
        }
        Ok(response)
    }

    pub fn receive(&self, kind: AccessType) -> SimResult {
        if kind == AccessType::ReadResponse {
            let bytes = self
                .payload
                .as_ref()
                .ok_or_else(|| SimError("functional read response has no payload".into()))?;
            if let Some(masks) = &self.masks {
                self.context
                    .local
                    .write_masked(self.address, bytes, masks)?;
            } else {
                self.context.local.write_bytes(self.address, bytes)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_masks_preserve_unaligned_word_and_page_boundaries() {
        for start in [0, 1, 62, 63, 64, 65, 127, 65535, 65536] {
            for length in [1, 2, 3, 63, 64, 65, 128, 129] {
                let memory = FunctionalMemory::default();
                let values = vec![0x5a; length];
                memory.write_bytes(start, &values).unwrap();
                assert_eq!(memory.read_bytes(start, length).unwrap(), values);
                if start > 0 {
                    let error = memory.read_bytes(start - 1, length + 1).unwrap_err();
                    assert!(error.to_string().contains(&format!("{:#x}", start - 1)));
                }
                let error = memory.read_bytes(start, length + 1).unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains(&format!("{:#x}", start + length as u64))
                );
            }
        }
        for hole in [0, 1, 37, 63, 64, 65, 100, 65535] {
            let memory = FunctionalMemory::default();
            memory.write_bytes(0, &vec![0x5a; hole]).unwrap();
            memory.write_bytes(hole as u64 + 1, &[0x6b; 69]).unwrap();
            let error = memory.read_bytes(0, hole + 70).unwrap_err();
            assert!(error.to_string().ends_with(&format!("{hole:#x}")));
            memory.write_bytes(hole as u64, &[0x7c]).unwrap();
            assert_eq!(memory.read_bytes(0, hole + 70).unwrap()[hole], 0x7c);
        }
    }

    #[test]
    fn packed_packets_preserve_bit_initialization_and_aliased_neighbors() {
        let backing = Rc::new(FunctionalMemory::default());
        backing
            .write_values(
                0,
                DataType::Int4,
                &[FunctionalValue::from_i64(DataType::Int4, 3).unwrap()],
            )
            .unwrap();
        let context = FunctionalContext::new(backing.clone());
        let response = context
            .packet(AccessType::ReadRequest, 0, 1)
            .unwrap()
            .respond(AccessType::ReadRequest)
            .unwrap();
        response.receive(AccessType::ReadResponse).unwrap();
        assert!(context.local.read_value(4, DataType::Int4).is_err());
        assert_eq!(
            context.local.read_value(0, DataType::Int4).unwrap().bits(),
            3
        );
        backing
            .write_values(
                0,
                DataType::Uint8,
                &[FunctionalValue::from_u64(DataType::Uint8, 0xa3).unwrap()],
            )
            .unwrap();
        let response = context
            .packet(AccessType::ReadRequest, 0, 1)
            .unwrap()
            .respond(AccessType::ReadRequest)
            .unwrap();
        response.receive(AccessType::ReadResponse).unwrap();
        context
            .write_value(0, FunctionalValue::from_i64(DataType::Int4, 5).unwrap())
            .unwrap();
        let write = context
            .packet(AccessType::WriteNonPostedRequest, 0, 1)
            .unwrap();
        backing
            .write_values(
                0,
                DataType::Uint8,
                &[FunctionalValue::from_u64(DataType::Uint8, 0xb3).unwrap()],
            )
            .unwrap();
        write.respond(AccessType::WriteNonPostedRequest).unwrap();
        assert_eq!(
            backing.read_values(0, DataType::Uint8, 1).unwrap()[0].bits(),
            0xb5
        );
    }

    #[test]
    #[ignore = "host payload-throughput diagnostic; run in release mode with --nocapture"]
    fn bulk_payload_throughput() {
        let values = vec![1.25; 1 << 22];
        let memory = FunctionalMemory::default();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            memory.write_f32(0, std::hint::black_box(&values)).unwrap();
            let written = started.elapsed();
            let started = std::time::Instant::now();
            std::hint::black_box(memory.read_f32(0, values.len()).unwrap());
            println!(
                "16MiB write_seconds={:.6} read_seconds={:.6}",
                written.as_secs_f64(),
                started.elapsed().as_secs_f64()
            );
        }
    }

    #[test]
    fn read_snapshot_arrives_with_response_and_write_mask_preserves_neighbors() {
        let backing = Rc::new(FunctionalMemory::default());
        backing.write_f32(0, &[1.0, 2.0, 3.0]).unwrap();
        let context = FunctionalContext::new(backing.clone());
        let request = context.packet(AccessType::ReadRequest, 4, 4).unwrap();
        assert!(context.local.read_f32(4, 1).is_err());
        let response = request.respond(AccessType::ReadRequest).unwrap();
        backing.write_f32(4, &[9.0]).unwrap();
        assert!(context.local.read_f32(4, 1).is_err());
        response.receive(AccessType::ReadResponse).unwrap();
        assert_eq!(context.local.read_f32(4, 1).unwrap(), [2.0]);
        context.local.write_f32(4, &[5.0]).unwrap();
        let write = context
            .packet(AccessType::WriteNonPostedRequest, 4, 4)
            .unwrap();
        assert_eq!(backing.read_f32(4, 1).unwrap(), [9.0]);
        write.respond(AccessType::WriteNonPostedRequest).unwrap();
        assert_eq!(backing.read_f32(0, 3).unwrap(), [1.0, 5.0, 3.0]);
    }
}
