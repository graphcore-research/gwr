// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Compact completed-work counters; independent of trace recording.
use std::collections::BTreeMap;

use gwr_engine::types::SimError;
use serde::{Deserialize, Serialize};

use super::{CapabilityKey, EngineBatch, EngineId};

/// A selected service action. Padding is not useful logical arithmetic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeAction {
    /// Exact operation and operand semantics used by the simulator.
    pub signature: CapabilityKey,
    /// Useful mathematical work.
    pub logical_operations: u64,
    /// Work charged by this engine's selected service shape.
    pub service_operations: u64,
}
/// Aggregate completed work for one engine/signature pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeCounterSnapshot {
    /// Stable engine name from the validated model.
    pub engine: String,
    /// Exact operation, input datatypes and optional execution semantics.
    pub signature: CapabilityKey,
    /// Completed useful operations.
    pub logical_operations: u64,
    /// Completed charged service operations, including declared padding.
    pub service_operations: u64,
}
/// Immutable simulator activity, bound to the exact resolved input snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeActivityReport {
    /// Current interchange format version.
    pub schema_version: u32,
    /// `gwr_native_activity`, distinguishing raw observations from mapped
    /// actions.
    pub kind: String,
    /// SHA-256 of the held include-resolved platform YAML.
    pub platform_digest: String,
    /// SHA-256 of the held include-resolved timetable YAML.
    pub workload_id: String,
    /// Actual elapsed simulation time, in seconds.
    pub duration_s: f64,
    /// Completed typed arithmetic by processing element.
    pub pes: Vec<NativePeActivity>,
    /// Boundary memory payload observations, not inferred physical bank
    /// actions.
    pub memory_payloads: Vec<NativeMemoryPayload>,
    /// Missing observation classes and limits on physical interpretation.
    pub coverage: Vec<String>,
}
/// One PE's complete aggregate compute observation interval.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePeActivity {
    /// Logical PE name.
    pub name: String,
    /// All enabled engine counters, including engines with no completed
    /// actions.
    pub engines: Vec<String>,
    /// Typed completed service counters.
    pub compute: Vec<ComputeCounterSnapshot>,
}
/// Payload at a logical memory boundary. Width/port/bank access counts are
/// unknown.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeMemoryPayload {
    /// Logical memory name.
    pub name: String,
    /// Returned payload bytes.
    pub read_bytes: u64,
    /// Committed payload bytes.
    pub written_bytes: u64,
}
/// Checked counters, updated once per completed engine batch.
#[derive(Debug, Default)]
pub struct ComputeCounters {
    counts: BTreeMap<(EngineId, CapabilityKey), (u64, u64)>,
}
impl ComputeCounters {
    /// Charge only the actually selected alternative, atomically on overflow.
    pub fn record_batch(&mut self, batch: &EngineBatch, engine: EngineId) -> Result<(), SimError> {
        let actions = batch
            .actions
            .get(&engine)
            .ok_or_else(|| SimError("selected engine lacks typed service actions".into()))?;
        let mut updates = BTreeMap::new();
        for a in actions {
            let key = (engine, a.signature.clone());
            let old = updates
                .get(&key)
                .or_else(|| self.counts.get(&key))
                .copied()
                .unwrap_or_default();
            let logical = old
                .0
                .checked_add(a.logical_operations)
                .ok_or_else(|| SimError("logical action counter overflow".into()))?;
            let service = old
                .1
                .checked_add(a.service_operations)
                .ok_or_else(|| SimError("service action counter overflow".into()))?;
            updates.insert(key, (logical, service));
        }
        self.counts.extend(updates);
        Ok(())
    }
    /// Stable typed snapshot; absent modes are not fabricated as observations.
    #[must_use]
    pub fn snapshot(&self, names: &[String]) -> Vec<ComputeCounterSnapshot> {
        self.counts
            .iter()
            .map(
                |((engine, signature), (logical, service))| ComputeCounterSnapshot {
                    engine: names
                        .get(engine.0)
                        .cloned()
                        .unwrap_or_else(|| format!("unknown-engine-{}", engine.0)),
                    signature: signature.clone(),
                    logical_operations: *logical,
                    service_operations: *service,
                },
            )
            .collect()
    }
}
