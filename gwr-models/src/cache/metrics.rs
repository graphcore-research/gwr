// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

use gwr_engine::time::compute_adjusted_value_and_rate;
use gwr_engine::types::AccessType;
use gwr_track::entity::Entity;
use gwr_track::info;

use crate::cache::coherency_manager::{CoherenceOp, CoherenceOpMetrics};

#[derive(Clone, Default)]
struct CacheMetricsData {
    payload_bytes_read: usize,
    payload_bytes_written: usize,
    num_read_hits: usize,
    num_read_pending_hits: usize,
    num_read_misses: usize,
    num_write_hits: usize,
    num_write_pending_hits: usize,
    num_write_misses: usize,
    coherence_ops_sent: CoherenceOpMetrics,
    coherence_ops_received: CoherenceOpMetrics,
}

#[derive(Default)]
pub(super) struct CacheMetrics {
    data: RefCell<CacheMetricsData>,
}

impl CacheMetrics {
    pub(super) fn payload_bytes_read(&self) -> usize {
        self.data.borrow().payload_bytes_read
    }

    pub(super) fn payload_bytes_written(&self) -> usize {
        self.data.borrow().payload_bytes_written
    }

    pub(super) fn num_hits(&self) -> usize {
        let data = self.data.borrow();
        data.num_read_hits
            + data.num_read_pending_hits
            + data.num_write_hits
            + data.num_write_pending_hits
    }

    pub(super) fn num_misses(&self) -> usize {
        let data = self.data.borrow();
        data.num_read_misses + data.num_write_misses
    }

    pub(super) fn record_payload_bytes_read(&self, bytes: usize) {
        self.data.borrow_mut().payload_bytes_read += bytes;
    }

    pub(super) fn record_completed_write_payload(&self, access_type: AccessType, bytes: usize) {
        if matches!(
            access_type,
            AccessType::WriteRequest | AccessType::WriteNonPostedRequest
        ) {
            self.data.borrow_mut().payload_bytes_written += bytes;
        }
    }

    pub(super) fn record_read_hit(&self) {
        self.data.borrow_mut().num_read_hits += 1;
    }

    pub(super) fn record_read_pending_hit(&self) {
        self.data.borrow_mut().num_read_pending_hits += 1;
    }

    pub(super) fn record_read_miss(&self) {
        self.data.borrow_mut().num_read_misses += 1;
    }

    pub(super) fn record_write_hit(&self) {
        self.data.borrow_mut().num_write_hits += 1;
    }

    pub(super) fn record_write_pending_hit(&self) {
        self.data.borrow_mut().num_write_pending_hits += 1;
    }

    pub(super) fn record_write_miss(&self) {
        self.data.borrow_mut().num_write_misses += 1;
    }

    pub(super) fn record_sent_coherence_op(&self, op: Option<CoherenceOp>) {
        if let Some(op) = op {
            self.data.borrow_mut().coherence_ops_sent.record(op);
        }
    }

    pub(super) fn record_received_coherence_op(&self, op: Option<CoherenceOp>) {
        if let Some(op) = op {
            self.data.borrow_mut().coherence_ops_received.record(op);
        }
    }

    pub(super) fn dump_stats(&self, entity: &Rc<Entity>, time_now_ns: f64) {
        let data = self.data.borrow();
        let (read_value, read_per_second) =
            compute_adjusted_value_and_rate(time_now_ns, data.payload_bytes_read);
        let (write_value, write_per_second) =
            compute_adjusted_value_and_rate(time_now_ns, data.payload_bytes_written);
        let num_hits = data.num_read_hits
            + data.num_read_pending_hits
            + data.num_write_hits
            + data.num_write_pending_hits;
        let num_misses = data.num_read_misses + data.num_write_misses;
        let total_accesses = num_hits + num_misses;
        let hit_rate = if total_accesses == 0 {
            0.0
        } else {
            num_hits as f64 / total_accesses as f64 * 100.0
        };

        info!(entity ; "Cache {}:", entity.full_name());
        info!(entity ;
            "  Read: {} bytes, {read_value:.2}, {read_per_second:.2}/s",
            data.payload_bytes_read
        );
        info!(entity ;
            "  Written: {} bytes, {write_value:.2}, {write_per_second:.2}/s",
            data.payload_bytes_written
        );
        info!(entity ;
            "  Read hits: {} actual, {} pending, Read misses: {}, Write hits: {} actual, {} pending, Write misses: {}, Hit rate: {hit_rate:.2}%",
            data.num_read_hits,
            data.num_read_pending_hits,
            data.num_read_misses,
            data.num_write_hits,
            data.num_write_pending_hits,
            data.num_write_misses
        );
        data.coherence_ops_sent.dump_stats(entity, "sent");
        data.coherence_ops_received.dump_stats(entity, "received");
    }
}

#[cfg(test)]
mod tests {
    use gwr_engine::test_helpers::start_test;

    use super::*;

    #[test]
    fn cache_metrics_record_queries_and_dump_stats() {
        let engine = start_test(file!());
        let entity = engine.top().clone();
        let metrics = CacheMetrics::default();

        assert_eq!(metrics.payload_bytes_read(), 0);
        assert_eq!(metrics.payload_bytes_written(), 0);
        assert_eq!(metrics.num_hits(), 0);
        assert_eq!(metrics.num_misses(), 0);
        metrics.dump_stats(&entity, 0.0);

        metrics.record_payload_bytes_read(64);
        metrics.record_completed_write_payload(AccessType::WriteRequest, 32);
        metrics.record_completed_write_payload(AccessType::ReadRequest, 128);
        metrics.record_read_hit();
        metrics.record_read_pending_hit();
        metrics.record_read_miss();
        metrics.record_write_hit();
        metrics.record_write_pending_hit();
        metrics.record_write_miss();
        metrics.record_sent_coherence_op(Some(CoherenceOp::SharedRead));
        metrics.record_received_coherence_op(Some(CoherenceOp::GrantShared));
        metrics.record_sent_coherence_op(None);
        metrics.record_received_coherence_op(None);

        assert_eq!(metrics.payload_bytes_read(), 64);
        assert_eq!(metrics.payload_bytes_written(), 32);
        assert_eq!(metrics.num_hits(), 4);
        assert_eq!(metrics.num_misses(), 2);
        metrics.dump_stats(&entity, 10.0);
    }
}
