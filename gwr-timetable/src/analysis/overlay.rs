// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Metric-overlay export for timetable visualisation tools.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use gwr_engine::time::clock::Clock;
use serde::Serialize;

use crate::analysis::roofline::RooflineReport;
use crate::analysis::ticks_to_ns;

const COMPUTE_NS: &str = "estimated_compute_ns";
const MEMORY_NS: &str = "estimated_memory_ns";
const SCHEDULED_FINISH_NS: &str = "estimated_scheduled_finish_ns";
const COMPUTE_EFFICIENCY: &str = "estimated_compute_efficiency";
const MEMORY_EFFICIENCY: &str = "estimated_memory_efficiency";

/// Description and unit for one exported metric.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MetricMetadata {
    /// Human-readable metric name.
    pub label: String,
    /// Unit displayed alongside metric values.
    pub unit: String,
}

/// Per-PE metrics in the JSON format consumed by `gwr-visualisation`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MetricOverlay {
    /// Metadata keyed by the metric names used in [`Self::metrics_by_pe`].
    pub metrics: BTreeMap<String, MetricMetadata>,
    /// Numeric metrics keyed first by platform PE name and then metric name.
    pub metrics_by_pe: BTreeMap<String, BTreeMap<String, f64>>,
}

impl MetricOverlay {
    /// Build an overlay from a completed roofline analysis.
    #[must_use]
    pub fn from_roofline_report(clock: &Clock, report: &RooflineReport) -> Self {
        let mut metrics = BTreeMap::from([
            metric(COMPUTE_NS, "Estimated compute time", "ns"),
            metric(MEMORY_NS, "Estimated memory time", "ns"),
        ]);
        let mut metrics_by_pe = report
            .pe_summaries
            .iter()
            .map(|summary| {
                let pe_metrics = BTreeMap::from([
                    (
                        COMPUTE_NS.to_string(),
                        nanoseconds(clock, summary.compute_ticks),
                    ),
                    (
                        MEMORY_NS.to_string(),
                        nanoseconds(clock, summary.memory_ticks),
                    ),
                ]);
                (summary.pe_name.clone(), pe_metrics)
            })
            .collect::<BTreeMap<_, _>>();

        if let Some(schedule) = &report.scheduled_activities {
            metrics.extend([
                metric(SCHEDULED_FINISH_NS, "Estimated scheduled finish time", "ns"),
                metric(COMPUTE_EFFICIENCY, "Estimated compute efficiency", "%"),
                metric(MEMORY_EFFICIENCY, "Estimated memory efficiency", "%"),
            ]);
            let mut finish_ticks_by_pe = BTreeMap::<String, f64>::new();
            for activity in &schedule.activities {
                let finish = finish_ticks_by_pe
                    .entry(activity.pe_name.clone())
                    .or_default();
                *finish = finish.max(activity.end_ticks);
            }
            for (pe_name, pe_metrics) in &mut metrics_by_pe {
                let finish_ticks = finish_ticks_by_pe.get(pe_name).copied().unwrap_or_default();
                pe_metrics.insert(
                    SCHEDULED_FINISH_NS.to_string(),
                    nanoseconds(clock, finish_ticks),
                );
                pe_metrics.insert(
                    COMPUTE_EFFICIENCY.to_string(),
                    efficiency(pe_metrics[COMPUTE_NS], pe_metrics[SCHEDULED_FINISH_NS]),
                );
                pe_metrics.insert(
                    MEMORY_EFFICIENCY.to_string(),
                    efficiency(pe_metrics[MEMORY_NS], pe_metrics[SCHEDULED_FINISH_NS]),
                );
            }
        }

        Self {
            metrics,
            metrics_by_pe,
        }
    }

    /// Write the overlay as formatted JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if the destination cannot be created or serialization
    /// fails.
    pub fn write_json(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let output = BufWriter::new(File::create(path)?);
        serde_json::to_writer_pretty(output, self)?;
        Ok(())
    }
}

fn metric(name: &str, label: &str, unit: &str) -> (String, MetricMetadata) {
    (
        name.to_string(),
        MetricMetadata {
            label: label.to_string(),
            unit: unit.to_string(),
        },
    )
}

fn nanoseconds(clock: &Clock, ticks: f64) -> f64 {
    let value = ticks_to_ns(clock, ticks);
    if value == 0.0 { 0.0 } else { value }
}

fn efficiency(work_ns: f64, scheduled_finish_ns: f64) -> f64 {
    if scheduled_finish_ns > 0.0 {
        work_ns / scheduled_finish_ns * 100.0
    } else {
        0.0
    }
}
