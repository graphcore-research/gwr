// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Deterministic address affinity and independent routed path configuration.

use gwr_engine::types::{SimError, SimResult};
use serde::{Deserialize, Serialize};

/// Canonical final memory identity and physical offset, shared by requests and
/// replies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FabricAddressKey {
    /// Stable configured memory ordinal, independent of runtime object IDs.
    pub memory_index: u64,
    /// Physical byte offset within that memory after address mapping.
    pub offset_in_memory: u64,
}

/// Caller-supplied pure address selector. Equal keys must always select the
/// same path.
pub trait FabricPathSelector {
    /// Return a path index below `count`, or reject the configuration.
    fn select(&self, key: FabricAddressKey, count: usize) -> Result<usize, SimError>;
}

/// Built-in, source-independent policies over canonical memory addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "policy", deny_unknown_fields)]
pub enum FabricPathSelection {
    /// XOR-fold address and memory identity into log2(count) bits.
    #[serde(rename = "address-xor-fold-v1")]
    AddressXorFold {
        /// Number of low byte-address bits ignored for affinity.
        address_shift: u8,
    },
    /// Fixed SplitMix64 finalization followed by modulo, supporting arbitrary
    /// counts.
    #[serde(rename = "address-mix-modulo-v1")]
    AddressMixModulo {
        /// Number of low byte-address bits ignored for affinity.
        address_shift: u8,
    },
}

impl FabricPathSelection {
    /// Size of the address region kept on one path.
    #[must_use]
    pub fn address_shift(self) -> u8 {
        match self {
            Self::AddressXorFold { address_shift } | Self::AddressMixModulo { address_shift } => {
                address_shift
            }
        }
    }
    /// Validate policy parameters without hashing an address.
    pub fn validate(self, count: usize) -> SimResult {
        if count == 0 || self.address_shift() > 63 {
            return Err(SimError(
                "fabric path count must be positive and address shift at most 63".into(),
            ));
        }
        if matches!(self, Self::AddressXorFold { .. }) && !count.is_power_of_two() {
            return Err(SimError(
                "address-xor-fold-v1 requires a power-of-two path count".into(),
            ));
        }
        Ok(())
    }
}

impl FabricPathSelector for FabricPathSelection {
    fn select(&self, key: FabricAddressKey, count: usize) -> Result<usize, SimError> {
        self.validate(count)?;
        if count == 1 {
            return Ok(0);
        }
        let line = key.offset_in_memory >> self.address_shift();
        Ok(match self {
            Self::AddressXorFold { .. } => {
                let bits = count.trailing_zeros();
                let fold = |mut value: u64| {
                    let mut result = 0;
                    while value != 0 {
                        result ^= value & (count as u64 - 1);
                        value >>= bits;
                    }
                    result as usize
                };
                fold(line) ^ fold(key.memory_index)
            }
            Self::AddressMixModulo { .. } => {
                (mix(line ^ mix(key.memory_index)) % count as u64) as usize
            }
        })
    }
}

// SplitMix64's fixed bijective finalizer; no process-local random seed.
fn mix(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// One throughput and queue recipe for every independent routed path.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FabricPathConfig {
    /// Number of independently queued mesh planes.
    pub count: usize,
    /// Bits per existing simulator clock tick, per directed path.
    pub bw_bits_per_tick: usize,
    /// Address-only path selection policy.
    pub selection: FabricPathSelection,
    /// Owned transport bytes per queue, including completed packets awaiting
    /// ACK.
    pub queue_bytes: usize,
    /// Owned packet bound, also covering packets with zero transport bytes.
    pub queue_packets: usize,
}
impl Default for FabricPathConfig {
    fn default() -> Self {
        Self {
            count: 1,
            bw_bits_per_tick: 256,
            selection: FabricPathSelection::AddressXorFold { address_shift: 6 },
            queue_bytes: 8192,
            queue_packets: 256,
        }
    }
}
impl FabricPathConfig {
    /// Reject invalid service/admission parameters before registering
    /// components.
    pub fn validate(&self) -> SimResult {
        self.selection.validate(self.count)?;
        if self.bw_bits_per_tick == 0 || self.queue_bytes == 0 || self.queue_packets == 0 {
            return Err(SimError(
                "fabric path bandwidth and queue bounds must be positive".into(),
            ));
        }
        self.count
            .checked_mul(self.queue_packets)
            .and_then(|n| n.checked_mul(self.queue_bytes))
            .ok_or_else(|| SimError("fabric path queue allocation overflows".into()))?;
        Ok(())
    }
}
