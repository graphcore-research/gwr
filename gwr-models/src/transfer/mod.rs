// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Bounded byte-preserving copies. Transfers use the ordinary memory protocol
//! and reserve neither compute engines nor whole tensors in compute SRAM.
use std::ops::Range;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};

use crate::processing_element::tensors::{HasShape, TensorView};

/// A transfer's issuing PE is selected by its timetable placement.
#[derive(Clone, Debug)]
pub struct TransferTaskConfig {
    pub id: String,
    pub source: TensorView,
    pub destination: TensorView,
    /// Maximum payload retained between each completed read and write.
    pub buffer_bytes: usize,
}

impl TransferTaskConfig {
    pub fn planned_traffic_bytes(&self) -> Result<(usize, usize), SimError> {
        self.validate()?;
        Ok((
            self.source.layout().num_access_bytes(),
            self.destination.layout().num_access_bytes(),
        ))
    }
    pub fn validate(&self) -> SimResult {
        if self.buffer_bytes == 0 {
            return sim_error!("Transfer '{}' buffer must be nonzero", self.id);
        }
        if self.source.dtype() != self.destination.dtype()
            || self.source.num_elements() != self.destination.num_elements()
        {
            return sim_error!(
                "Transfer '{}' source and destination must have equal dtype and element count",
                self.id
            );
        }
        if self.source.layout().num_access_bytes() != self.destination.layout().num_access_bytes() {
            return sim_error!(
                "Transfer '{}' physical byte counts differ (packed views require matching byte alignment)",
                self.id
            );
        }
        if self
            .source
            .first_overlapping_byte_ranges(&self.destination)
            .is_some()
        {
            return sim_error!("Transfer '{}' source and destination overlap", self.id);
        }
        Ok(())
    }

    /// Lazy pairs of physical ranges, bounded by the configured buffer.
    /// Differently strided byte-aligned views are paired in logical row order.
    pub fn chunks(&self) -> impl Iterator<Item = (Range<u128>, Range<u128>)> + '_ {
        let mut source = self.source.address_ranges();
        let mut destination = self.destination.address_ranges();
        let mut src = source.next();
        let mut dst = destination.next();
        std::iter::from_fn(move || {
            let (s, d) = (src.as_mut()?, dst.as_mut()?);
            let bytes = (s.end - s.start)
                .min(d.end - d.start)
                .min(self.buffer_bytes as u128);
            let pair = (s.start..s.start + bytes, d.start..d.start + bytes);
            s.start += bytes;
            d.start += bytes;
            if s.start == s.end {
                src = source.next();
            }
            if d.start == d.end {
                dst = destination.next();
            }
            Some(pair)
        })
    }
}
