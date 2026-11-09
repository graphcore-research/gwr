// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Footprint planning without per-transaction expansion.
//!
//! Exact planning visits lazy physical ranges; analytical planning summarizes
//! byte-aligned repeated ranges and contiguous packed coverage by map boundary.
//! No engine work, routing, admission or cache/boundary dependencies are
//! created.

use std::collections::BTreeMap;
use std::rc::Rc;

use gwr_engine::types::{DeviceId, SimError};

use super::accounting::{Operation, Traffic, TransactionRun, account, split_range};
use crate::memory::memory_map::{MemoryMap, MemoryRegion};
use crate::processing_element::tensors::TensorView;

mod analytical;

/// One destination's exact demand summary. Uniform runs are sorted by payload
/// size; this is not transaction/address order. Headers are preserved when
/// equal transaction sizes are grouped, never recomputed from aggregate
/// payload.
#[derive(Clone, Debug)]
pub struct DestinationTraffic {
    device: DeviceId,
    runs: Vec<TransactionRun>,
    traffic: Traffic,
}

impl DestinationTraffic {
    #[must_use]
    pub fn device(&self) -> DeviceId {
        self.device
    }

    #[must_use]
    pub fn runs(&self) -> &[TransactionRun] {
        &self.runs
    }

    #[must_use]
    pub fn traffic(&self) -> Traffic {
        self.traffic
    }
}

/// One view occurrence and its destination summaries. The view, map snapshot
/// and shaping parameters retain enough provenance to reconstruct boundaries.
/// A consumer must not use sorted summary runs as an ordered address ledger.
#[derive(Clone, Debug)]
pub struct ViewTraffic {
    view: TensorView,
    regions: Vec<MemoryRegion>,
    map: Rc<MemoryMap>,
    operation: Operation,
    maximum: u128,
    header: u128,
    destinations: Vec<DestinationTraffic>,
}

/// One consecutive physical span, bounded by the view and address translation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappedRange {
    pub device: DeviceId,
    pub physical_address: u64,
    pub offset_in_device: u64,
    pub payload_bytes: u128,
}

impl ViewTraffic {
    /// Count byte-aligned strided ranges without visiting each fragment. Also
    /// supports physically contiguous packed coverage. Fragmented packed views
    /// are explicitly rejected, never silently sent to the slower exact path.
    /// Unstriped work scales with tensor rank and map boundaries. Dense striped
    /// coverage scales with member count; fragmented striped layouts are
    /// supported up to 100,000 view ranges and explicitly rejected beyond it.
    pub fn new_analytical(
        view: &TensorView,
        operation: Operation,
        map: &MemoryMap,
        maximum: u128,
        header: u128,
    ) -> Result<Self, SimError> {
        if maximum == 0 {
            return Err(SimError(
                "Roofline maximum transaction payload must be positive".into(),
            ));
        }
        let counts = if map.has_striping() {
            analytical::summarize_striped(view, map, maximum)?
        } else {
            analytical::summarize(view, &map.regions().cloned().collect::<Vec<_>>(), maximum)?
        };
        let mut result = Self::mapped(view, operation, map, maximum, header)?;
        result.set_counts(counts)?;
        Ok(result)
    }

    /// Split physical ranges at mapped region boundaries before shaping. An
    /// unmapped touched byte fails the entire plan; untouched gaps are ignored.
    /// Region ends use u128 exclusive arithmetic to support address u64::MAX.
    pub fn new_exact(
        view: &TensorView,
        operation: Operation,
        map: &MemoryMap,
        maximum: u128,
        header: u128,
    ) -> Result<Self, SimError> {
        if maximum == 0 {
            return Err(SimError(
                "Roofline maximum transaction payload must be positive".into(),
            ));
        }
        let mut result = Self::mapped(view, operation, map, maximum, header)?;
        let mut counts: BTreeMap<u64, BTreeMap<u128, u128>> = BTreeMap::new();
        for range in result.mapped_ranges() {
            let range = range?;
            for run in split_range(range.payload_bytes, maximum, header)
                .map_err(|e| SimError(format!("Roofline transaction shaping failed: {e:?}")))?
            {
                let count = counts
                    .entry(range.device.0)
                    .or_default()
                    .entry(run.payload_bytes)
                    .or_default();
                *count = count
                    .checked_add(run.count)
                    .ok_or_else(|| SimError("Roofline transaction count overflow".into()))?;
            }
        }
        result.set_counts(counts)?;
        Ok(result)
    }

    /// Retain a map snapshot without eagerly traversing the view or computing
    /// destination summaries. Consumers must fully traverse `mapped_ranges`
    /// and validate all results before submitting any work. `destinations()`
    /// is empty for this constructor; summary constructors populate it.
    pub fn mapped(
        view: &TensorView,
        operation: Operation,
        map: &MemoryMap,
        maximum: u128,
        header: u128,
    ) -> Result<Self, SimError> {
        if maximum == 0 {
            return Err(SimError(
                "Roofline maximum transaction payload must be positive".into(),
            ));
        }
        Ok(Self {
            view: view.clone(),
            regions: map.physical_regions().cloned().collect(),
            map: Rc::new(map.clone()),
            operation,
            maximum,
            header,
            destinations: Vec::new(),
        })
    }

    /// Lazily translate view ranges into physical spans. Each span ends at a
    /// view or mapping boundary, independent of transaction payload limits.
    /// The iterator stops after the first unmapped touched byte.
    pub fn mapped_ranges(&self) -> impl Iterator<Item = Result<MappedRange, SimError>> + '_ {
        let mut ranges = self.view.address_ranges();
        let mut current = None;
        let mut failed = false;
        std::iter::from_fn(move || {
            if failed {
                return None;
            }
            let range = current.get_or_insert_with(|| ranges.next().unwrap_or(0..0));
            if range.start == range.end {
                *range = ranges.next()?;
            }
            let address = u64::try_from(range.start).ok();
            let Some(resolved) = address.and_then(|address| self.map.resolve(address)) else {
                failed = true;
                return Some(Err(SimError(format!(
                    "Unmapped roofline physical address {:#x}",
                    range.start
                ))));
            };
            let payload_bytes = (range.end - range.start).min(resolved.contiguous_bytes);
            range.start += payload_bytes;
            Some(Ok(MappedRange {
                device: resolved.device,
                physical_address: resolved.physical_address,
                offset_in_device: resolved.offset_in_device,
                payload_bytes,
            }))
        })
    }

    fn set_counts(&mut self, counts: BTreeMap<u64, BTreeMap<u128, u128>>) -> Result<(), SimError> {
        let header = self.header;
        let destinations = counts
            .into_iter()
            .map(|(device, counts)| {
                let runs: Vec<_> = counts
                    .into_iter()
                    .map(|(payload_bytes, count)| TransactionRun {
                        payload_bytes,
                        count,
                        request_header_bytes: header,
                        response_header_bytes: header,
                    })
                    .collect();
                let traffic = account(self.operation, &runs)
                    .map_err(|e| SimError(format!("Roofline traffic accounting failed: {e:?}")))?;
                Ok(DestinationTraffic {
                    device: DeviceId(device),
                    runs,
                    traffic,
                })
            })
            .collect::<Result<_, SimError>>()?;
        self.destinations = destinations;
        Ok(())
    }

    #[must_use]
    pub fn view(&self) -> &TensorView {
        &self.view
    }

    /// Physical member regions retained in the map snapshot.
    #[must_use]
    pub fn regions(&self) -> &[MemoryRegion] {
        &self.regions
    }

    #[must_use]
    pub fn operation(&self) -> Operation {
        self.operation
    }

    #[must_use]
    pub fn maximum_payload(&self) -> u128 {
        self.maximum
    }

    #[must_use]
    pub fn header_bytes(&self) -> u128 {
        self.header
    }

    /// Children are in stable device-ID order, independent of address order.
    #[must_use]
    pub fn destinations(&self) -> &[DestinationTraffic] {
        &self.destinations
    }
}
