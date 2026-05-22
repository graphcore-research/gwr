// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::time::Instant;

use gwr_engine::types::SimError;
use gwr_platform::Platform;
use gwr_platform::types::PlatformConfig;

use crate::analysis::ComputeNodeAnalysis;
use crate::analysis::memory::{BandwidthGraph, WidestPathCache, resource_bytes_per_cycle};
use crate::analysis::pe::{
    ComputeNodeRoofline, CriticalPathAnalysis, PeRooflineSummary, ScheduledActivities,
    aggregate_pe_rooflines, compute_node_rooflines, critical_path_analysis, schedule_pe_activities,
};
use crate::{Timetable, TimetableStats};

/// Runs the approximate roofline model for a validated timetable/platform pair.
///
/// Modelling assumptions:
/// - Each compute node is costed as `max(compute_ticks, memory_ticks)`.
/// - Tensor views must fit within a single memory range.
/// - Memory traffic uses widest-path bandwidth between the assigned PE and each
///   accessed memory, including PE LSU overhead.
/// - The contention pass approximates shared-memory pressure from overlapping
///   PE activities; it is intended for coarse ranking and comparison, not
///   cycle-accurate simulation.
pub struct RooflineAnalyzer {
    bandwidth_graph: BandwidthGraph,
}

#[derive(Clone, Copy, Debug)]
pub struct RooflineAnalysisOptions {
    pub schedule_activities: bool,
}

impl Default for RooflineAnalysisOptions {
    fn default() -> Self {
        Self {
            schedule_activities: true,
        }
    }
}

#[derive(Debug)]
pub struct RooflineReport {
    pub timetable_stats: TimetableStats,
    pub compute_nodes: Vec<ComputeNodeAnalysis>,
    pub node_rooflines: Vec<ComputeNodeRoofline>,
    pub pe_summaries: Vec<PeRooflineSummary>,
    pub scheduled_activities: Option<ScheduledActivities>,
    pub critical_path: CriticalPathAnalysis,
    pub pe_lower_bound_ticks: f64,
    pub estimated_best_case_ticks: f64,
    pub scheduled_runtime_ticks: Option<f64>,
}

impl RooflineAnalyzer {
    pub fn new(
        platform: &Platform,
        platform_cfg: &PlatformConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let bytes_per_cycle = resource_bytes_per_cycle(platform)?;
        let bandwidth_graph = BandwidthGraph::build(platform_cfg, &bytes_per_cycle)?;
        Ok(Self { bandwidth_graph })
    }

    pub fn analyze(
        &self,
        platform: &Platform,
        timetable: &Timetable,
    ) -> Result<RooflineReport, Box<dyn std::error::Error>> {
        self.analyze_with_options(platform, timetable, RooflineAnalysisOptions::default())
    }

    pub fn analyze_with_options(
        &self,
        platform: &Platform,
        timetable: &Timetable,
        options: RooflineAnalysisOptions,
    ) -> Result<RooflineReport, Box<dyn std::error::Error>> {
        let timings = std::env::var_os("GWR_ROOFLINE_TIMINGS").is_some();
        let mut phase_start = Instant::now();
        let timetable_stats = timetable.stats()?;
        if timings {
            eprintln!(
                "timing: analyse.stats {:.3}s",
                phase_start.elapsed().as_secs_f64()
            );
        }

        phase_start = Instant::now();
        let compute_nodes = timetable.compute_node_analyses()?;
        if timings {
            eprintln!(
                "timing: analyse.extract_compute_nodes {:.3}s",
                phase_start.elapsed().as_secs_f64()
            );
        }
        if compute_nodes.is_empty() {
            return Err(SimError("Timetable contains no compute nodes".to_string()).into());
        }

        let mut widest_path_cache = WidestPathCache::default();
        phase_start = Instant::now();
        let node_rooflines = compute_node_rooflines(
            platform,
            &compute_nodes,
            &self.bandwidth_graph,
            &mut widest_path_cache,
        )?;
        if timings {
            eprintln!(
                "timing: analyse.compute_node_rooflines {:.3}s",
                phase_start.elapsed().as_secs_f64()
            );
        }

        phase_start = Instant::now();
        let pe_summaries = aggregate_pe_rooflines(
            platform,
            &node_rooflines,
            &self.bandwidth_graph,
            &mut widest_path_cache,
        )?;
        if timings {
            eprintln!(
                "timing: analyse.aggregate_pe_rooflines {:.3}s",
                phase_start.elapsed().as_secs_f64()
            );
        }

        let scheduled_activities = if options.schedule_activities {
            phase_start = Instant::now();
            let scheduled_activities = schedule_pe_activities(
                platform,
                &node_rooflines,
                &self.bandwidth_graph,
                &mut widest_path_cache,
            )?;
            if timings {
                eprintln!(
                    "timing: analyse.schedule_pe_activities {:.3}s",
                    phase_start.elapsed().as_secs_f64()
                );
            }
            Some(scheduled_activities)
        } else {
            if timings {
                eprintln!("timing: analyse.schedule_pe_activities skipped");
            }
            None
        };

        phase_start = Instant::now();
        let critical_path = critical_path_analysis(&node_rooflines)?;
        if timings {
            eprintln!(
                "timing: analyse.critical_path {:.3}s",
                phase_start.elapsed().as_secs_f64()
            );
        }
        let pe_lower_bound_ticks = pe_summaries
            .iter()
            .map(|summary| summary.roofline_ticks)
            .fold(0.0, f64::max);
        let estimated_best_case_ticks = critical_path.total_ticks.max(pe_lower_bound_ticks);
        let scheduled_runtime_ticks = scheduled_activities.as_ref().map(|scheduled_activities| {
            scheduled_activities
                .activities
                .iter()
                .map(|activity| activity.end_ticks)
                .fold(0.0, f64::max)
        });

        Ok(RooflineReport {
            timetable_stats,
            compute_nodes,
            node_rooflines,
            pe_summaries,
            scheduled_activities,
            critical_path,
            pe_lower_bound_ticks,
            estimated_best_case_ticks,
            scheduled_runtime_ticks,
        })
    }
}
