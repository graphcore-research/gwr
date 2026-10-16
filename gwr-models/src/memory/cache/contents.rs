// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Cache contents, validity, and ownership of parent requests and fills.
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gwr_engine::traits::SimObject;
use gwr_engine::types::{AccessType, SimError};

use super::{CacheConfig, CacheInflightFills, CacheMetrics};
use crate::memory::CacheHintType;
use crate::memory::traits::{CacheAccess, CacheFillToken};

#[derive(Clone, Copy, Default)]
enum Validity {
    #[default]
    Missing,
    Valid,
    Pending(u64),
}
struct Entry {
    tag: Option<u64>,
    validity: Validity,
}
struct Parent<T> {
    request: T,
    pending: BTreeSet<u64>,
    issued: bool,
    miss: bool,
}

struct Fill {
    waiters: BTreeSet<u64>,
    install: Option<(usize, usize)>,
    address: u64,
    bytes: usize,
}
pub(super) enum LineAction<T> {
    Advance(u128),
    Send(T, u128),
    Blocked,
}
pub(super) struct CacheContents<T: SimObject + CacheAccess> {
    pub(super) config: CacheConfig,
    sets: Vec<Vec<Entry>>,
    victims: Vec<usize>,
    parents: BTreeMap<u64, Parent<T>>,
    fills: BTreeMap<u64, Fill>,
    owner: Rc<()>,
    peak_inflight_fills: usize,
    next_parent: u64,
    next_fill: u64,
    pub(super) response_service_end: u64,
}
impl<T: SimObject + CacheAccess> CacheContents<T> {
    pub(super) fn new(config: CacheConfig) -> Self {
        let sets = (0..config.num_sets)
            .map(|_| {
                (0..config.num_ways)
                    .map(|_| Entry {
                        tag: None,
                        validity: Validity::Missing,
                    })
                    .collect()
            })
            .collect();
        let victims = vec![0; config.num_sets];
        Self {
            config,
            sets,
            victims,
            parents: BTreeMap::new(),
            fills: BTreeMap::new(),
            owner: Rc::new(()),
            peak_inflight_fills: 0,
            next_parent: 0,
            next_fill: 0,
            response_service_end: 0,
        }
    }

    pub(super) fn inflight_fills_snapshot(&self) -> CacheInflightFills {
        CacheInflightFills {
            current: self.fills.len(),
            peak: self.peak_inflight_fills,
        }
    }

    pub(super) fn admit_read_request(&mut self, request: T) -> Result<(u64, u128, u128), SimError> {
        if request.access_type() != AccessType::ReadRequest {
            return Err(error("cache read admission requires a read request"));
        }
        let end = Self::checked_range(request.dst_addr(), request.access_size_bytes())?;
        let start = self.line_start(request.dst_addr());
        // Widened exclusive endpoints represent the legitimate final byte
        // u64::MAX; child packets retain inclusive checked u64 ranges.
        let id = self.next_parent;
        self.next_parent = id
            .checked_add(1)
            .ok_or_else(|| error("cache parent identity exhausted"))?;
        if !self.has_parent_capacity() {
            return Err(error("cache parent admission invariant"));
        }
        self.parents.insert(
            id,
            Parent {
                request,
                pending: BTreeSet::new(),
                issued: false,
                miss: false,
            },
        );
        Ok((id, start as u128, end))
    }

    /// Check or fetch one whole cache line.
    /// Memory fills are bounded separately from parent requests.
    pub(super) fn process_line(
        &mut self,
        parent: u64,
        cursor: u128,
        _end: u128,
        metrics: &mut CacheMetrics,
    ) -> Result<LineAction<T>, SimError> {
        let address =
            u64::try_from(cursor).map_err(|_| error("cache cursor outside address space"))?;
        let (set, tag) = self.location(address);
        let allocate = self.parents[&parent].request.cache_hint() == CacheHintType::Allocate;
        let way = self.way(set, tag);
        let validity = way.map_or(Validity::Missing, |w| self.sets[set][w].validity);
        let next = cursor + self.config.line_size_bytes as u128;
        match validity {
            Validity::Valid => {
                metrics.line_hits += 1;
                return Ok(LineAction::Advance(next));
            }
            Validity::Pending(fill) if allocate => {
                self.parents.get_mut(&parent).unwrap().miss = true;
                self.parents.get_mut(&parent).unwrap().pending.insert(fill);
                self.fills
                    .get_mut(&fill)
                    .ok_or_else(|| error("cache pending line lost fill"))?
                    .waiters
                    .insert(parent);
                metrics.coalesced_lines += 1;
                return Ok(LineAction::Advance(next));
            }
            _ => {}
        }
        if self.fills.len() >= self.config.max_inflight_fills {
            return Ok(LineAction::Blocked);
        }
        let bytes = self.config.line_size_bytes;
        let next = Self::checked_range(address, bytes)?;
        let id = self.next_fill;
        let next_id = id
            .checked_add(1)
            .ok_or_else(|| error("cache fill identity exhausted"))?;
        let total_occupancy = self
            .fills
            .len()
            .checked_add(1)
            .ok_or_else(|| error("cache global occupancy overflows"))?;
        let line_read_request = self.parents[&parent].request.cache_read_fragment(
            address,
            bytes,
            CacheFillToken {
                owner: self.owner.clone(),
                ordinal: id,
            },
        )?;
        // Unsupported packet fragmentation/range errors occurred before any
        // fill/validity mutation. Each child has a checked component identity.
        self.next_fill = next_id;
        let install = if allocate {
            let way = self.reserve_line(set, tag)?;
            self.sets[set][way].validity = Validity::Pending(id);
            Some((set, way))
        } else {
            None
        };
        self.fills.insert(
            id,
            Fill {
                waiters: BTreeSet::from([parent]),
                install,
                address,
                bytes,
            },
        );
        self.peak_inflight_fills = self.peak_inflight_fills.max(total_occupancy);
        let parent_state = self.parents.get_mut(&parent).unwrap();
        parent_state.pending.insert(id);
        parent_state.miss = true;
        metrics.line_misses += 1;
        metrics.fill_requests += 1;
        metrics.downstream_read_bytes += bytes;
        metrics.downstream_request_header_bytes += line_read_request.total_bytes();
        Ok(LineAction::Send(line_read_request, next))
    }

    pub(super) fn finish_issuing(&mut self, parent: u64, metrics: &mut CacheMetrics) -> Option<T> {
        let parent_state = self.parents.get_mut(&parent).unwrap();
        parent_state.issued = true;
        if parent_state.miss {
            metrics.num_misses += 1;
        } else {
            metrics.num_hits += 1;
        }
        if parent_state.pending.is_empty() {
            Some(self.parents.remove(&parent).unwrap().request)
        } else {
            None
        }
    }

    pub(super) fn response(&mut self, response: &T) -> Result<Vec<T>, SimError> {
        let token = response
            .cache_fill_token()
            .ok_or_else(|| error("cache response lacks internal fill token"))?;
        if !Rc::ptr_eq(&token.owner, &self.owner) {
            return Err(error("cache response belongs to another fill owner"));
        }
        let fill = self
            .fills
            .get(&token.ordinal)
            .ok_or_else(|| error("unknown/duplicate cache fill response"))?;
        if response.dst_addr() != fill.address || response.access_size_bytes() != fill.bytes {
            return Err(error("cache fill response range mismatch"));
        }
        // Validate the entire ownership transition before any slot/validity
        // mutation. Failed or duplicate responses never release capacity.
        for parent in &fill.waiters {
            let parent_state = self
                .parents
                .get(parent)
                .ok_or_else(|| error("cache fill lost parent"))?;
            if !parent_state.pending.contains(&token.ordinal) {
                return Err(error("cache fill parent dependency mismatch"));
            }
        }
        let fill = self.fills.remove(&token.ordinal).unwrap();
        if let Some((set, way)) = fill.install {
            let entry = &mut self.sets[set][way];
            // Fill IDs are never reused. Replacement, invalidation, or a newer
            // fill changes this state, so an obsolete response cannot install.
            if matches!(entry.validity, Validity::Pending(id) if id == token.ordinal) {
                entry.validity = Validity::Valid;
            }
        }
        let mut ready = vec![];
        for parent in fill.waiters {
            let parent_state = self.parents.get_mut(&parent).unwrap();
            parent_state.pending.remove(&token.ordinal);
            if parent_state.issued && parent_state.pending.is_empty() {
                ready.push(self.parents.remove(&parent).unwrap().request);
            }
        }
        Ok(ready)
    }

    /// Writes invalidate every overlapping line, including pending fills.
    /// Point controls invalidate the entire containing tag line.
    pub(super) fn invalidate_range(&mut self, address: u64, bytes: usize) -> Result<(), SimError> {
        let line_size = self.config.line_size_bytes as u128;
        let mut cursor = self.line_start(address) as u128;
        let end = if bytes == 0 {
            (cursor + line_size).min(u64::MAX as u128 + 1)
        } else {
            Self::checked_range(address, bytes)?
        };
        while cursor < end {
            let (set, tag) = self.location(
                u64::try_from(cursor)
                    .map_err(|_| error("cache invalidation cursor outside address space"))?,
            );
            if let Some(way) = self.way(set, tag) {
                self.sets[set][way].validity = Validity::Missing;
            }
            cursor += line_size;
        }
        Ok(())
    }

    pub(super) fn has_parent_capacity(&self) -> bool {
        self.parents.len() < self.config.max_inflight_parents
    }

    pub(super) fn checked_range(address: u64, bytes: usize) -> Result<u128, SimError> {
        if bytes == 0 {
            return Err(error("zero-size cache read/write"));
        }
        let end = address as u128 + bytes as u128;
        if end > u64::MAX as u128 + 1 {
            return Err(error("cache address range overflows"));
        }
        Ok(end)
    }

    fn reserve_line(&mut self, set: usize, tag: u64) -> Result<usize, SimError> {
        if let Some(way) = self.way(set, tag) {
            return Ok(way);
        }
        let way = self.victims[set];
        self.victims[set] = (way + 1) % self.config.num_ways;
        let entry = &mut self.sets[set][way];
        entry.tag = Some(tag);
        entry.validity = Validity::Missing;
        Ok(way)
    }

    fn way(&self, set: usize, tag: u64) -> Option<usize> {
        self.sets[set].iter().position(|e| e.tag == Some(tag))
    }

    fn line_start(&self, address: u64) -> u64 {
        let line_size = self.config.line_size_bytes as u64;
        address - address % line_size
    }

    fn location(&self, address: u64) -> (usize, u64) {
        let line = address / self.config.line_size_bytes as u64;
        (
            (line % self.config.num_sets as u64) as usize,
            line / self.config.num_sets as u64,
        )
    }
}

fn error(message: &str) -> SimError {
    SimError(message.into())
}

#[cfg(test)]
mod tests;
