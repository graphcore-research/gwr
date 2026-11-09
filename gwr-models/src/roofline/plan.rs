// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Exact aggregate resource demands for one planned service segment.
//!
//! Topology compilers select resources; this module does not infer routes,
//! capacities, addresses, cache branches or adapter completion dependencies.
//! A plan is a demand summary, not a replacement for transaction provenance.
use std::collections::{BTreeMap, BTreeSet};

use gwr_engine::types::SimError;

use super::accounting::{Operation, Traffic, TransactionRun, account};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DemandKind {
    Payload,
    RequestWire,
    ResponseWire,
}

/// Charge one demand kind once to a resource in the target domain's table.
/// Separate resources model duplex service; different kinds on the same
/// resource model shared capacity. Duplicate resource/kind pairs are errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceUse {
    pub resource: usize,
    pub kind: DemandKind,
}

#[derive(Clone, Debug)]
pub struct ServicePlan {
    traffic: Traffic,
    resource_demands: Vec<(usize, u128)>,
    latency_ticks: u64,
}

impl ServicePlan {
    /// Build an immutable plan, validating every use against `resource_count`.
    /// Uses with zero demand are validated but omitted from service. Nonempty
    /// payload must have at least one positive charge. Header-only traffic is
    /// unsupported by this payload-coordinate plan; use a distinct coordinate
    /// when implementing control/response-only stages.
    ///
    /// `latencies` are additive completion tails in the platform clock.
    pub fn new(
        operation: Operation,
        runs: &[TransactionRun],
        uses: &[ResourceUse],
        resource_count: usize,
        latencies: &[u64],
    ) -> Result<Self, SimError> {
        let traffic = account(operation, runs)
            .map_err(|error| SimError(format!("roofline service accounting: {error:?}")))?;
        if traffic.payload == 0 && (traffic.request_wire != 0 || traffic.response_wire != 0) {
            return Err(SimError(
                "roofline header-only service needs a non-payload progress coordinate".into(),
            ));
        }
        let mut owned = BTreeSet::new();
        let mut demands = BTreeMap::<usize, u128>::new();
        for &usage in uses {
            if usage.resource >= resource_count {
                return Err(SimError(format!(
                    "roofline resource {} is outside table of size {resource_count}",
                    usage.resource
                )));
            }
            if !owned.insert(usage) {
                return Err(SimError(format!(
                    "duplicate roofline charge for resource {} and {:?}",
                    usage.resource, usage.kind
                )));
            }
            let bytes = match usage.kind {
                DemandKind::Payload => traffic.payload,
                DemandKind::RequestWire => traffic.request_wire,
                DemandKind::ResponseWire => traffic.response_wire,
            };
            if bytes != 0 {
                let demand = demands.entry(usage.resource).or_default();
                *demand = demand.checked_add(bytes).ok_or_else(|| {
                    SimError(format!(
                        "roofline demand overflow on resource {}",
                        usage.resource
                    ))
                })?;
            }
        }
        if traffic.payload != 0 && demands.is_empty() {
            return Err(SimError(
                "roofline nonempty service has no charged resource".into(),
            ));
        }
        let latency_ticks = latencies.iter().try_fold(0_u64, |sum, &latency| {
            sum.checked_add(latency)
                .ok_or_else(|| SimError("roofline path latency overflow".into()))
        })?;
        Ok(Self {
            traffic,
            resource_demands: demands.into_iter().collect(),
            latency_ticks,
        })
    }

    #[must_use]
    pub fn traffic(&self) -> Traffic {
        self.traffic
    }

    /// Exact resource byte totals, sorted by domain resource index.
    #[must_use]
    pub fn resource_demands(&self) -> &[(usize, u128)] {
        &self.resource_demands
    }

    /// Aggregate completion tail for the compiled route.
    #[must_use]
    pub fn latency_ticks(&self) -> u64 {
        self.latency_ticks
    }
}
