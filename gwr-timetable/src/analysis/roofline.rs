// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::time::Instant;

use gwr_engine::types::SimError;
use gwr_platform::Platform;
use gwr_platform::types::PlatformConfig;

use crate::analysis::ComputeNodeAnalysis;
use crate::analysis::cache::{CacheModel, CacheSharingReport, apply_cache_model};
use crate::analysis::memory::{BandwidthGraph, WidestPathCache, resource_bytes_per_cycle};
use crate::analysis::pe::{
    ComputeNodeRoofline, CriticalPathAnalysis, PeRooflineSummary, ScheduledActivities,
    aggregate_pe_rooflines, compute_node_rooflines, critical_path_analysis, schedule_pe_activities,
};
use crate::{Timetable, TimetableStats};

type RooflineResult<T> = Result<T, Box<dyn std::error::Error>>;

struct AnalysisTimer {
    enabled: bool,
}

impl AnalysisTimer {
    fn from_env() -> Self {
        Self {
            enabled: std::env::var_os("GWR_ROOFLINE_TIMINGS").is_some(),
        }
    }

    fn time_result<T, E>(&self, name: &str, f: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
        let phase_start = Instant::now();
        let value = f()?;
        if self.enabled {
            eprintln!("timing: {name} {:.3}s", phase_start.elapsed().as_secs_f64());
        }
        Ok(value)
    }

    fn time_value<T>(&self, name: &str, f: impl FnOnce() -> T) -> T {
        let phase_start = Instant::now();
        let value = f();
        if self.enabled {
            eprintln!("timing: {name} {:.3}s", phase_start.elapsed().as_secs_f64());
        }
        value
    }

    fn skipped(&self, name: &str) {
        if self.enabled {
            eprintln!("timing: {name} skipped");
        }
    }
}

struct RooflineEstimates {
    pe_lower_bound_ticks: f64,
    estimated_best_case_ticks: f64,
    scheduled_runtime_ticks: Option<f64>,
}

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
    platform_cfg: PlatformConfig,
}

#[derive(Clone, Copy, Debug)]
pub struct RooflineAnalysisOptions {
    pub schedule_activities: bool,
    pub cache_model: CacheModel,
}

impl Default for RooflineAnalysisOptions {
    fn default() -> Self {
        Self {
            schedule_activities: true,
            cache_model: CacheModel::Worst,
        }
    }
}

#[derive(Debug)]
pub struct RooflineReport {
    pub timetable_stats: TimetableStats,
    pub compute_nodes: Vec<ComputeNodeAnalysis>,
    pub cache_sharing: CacheSharingReport,
    pub node_rooflines: Vec<ComputeNodeRoofline>,
    pub pe_summaries: Vec<PeRooflineSummary>,
    pub scheduled_activities: Option<ScheduledActivities>,
    pub critical_path: CriticalPathAnalysis,
    pub pe_lower_bound_ticks: f64,
    pub estimated_best_case_ticks: f64,
    pub scheduled_runtime_ticks: Option<f64>,
}

impl RooflineAnalyzer {
    pub fn new(platform: &Platform, platform_cfg: &PlatformConfig) -> RooflineResult<Self> {
        let bytes_per_cycle = resource_bytes_per_cycle(platform)?;
        let bandwidth_graph = BandwidthGraph::build(platform_cfg, &bytes_per_cycle)?;
        Ok(Self {
            bandwidth_graph,
            platform_cfg: platform_cfg.clone(),
        })
    }

    pub fn analyze(
        &self,
        platform: &Platform,
        timetable: &Timetable,
    ) -> RooflineResult<RooflineReport> {
        self.analyze_with_options(platform, timetable, RooflineAnalysisOptions::default())
    }

    pub fn analyze_with_options(
        &self,
        platform: &Platform,
        timetable: &Timetable,
        options: RooflineAnalysisOptions,
    ) -> RooflineResult<RooflineReport> {
        let timer = AnalysisTimer::from_env();
        let timetable_stats = timer.time_result("analyse.stats", || timetable.stats())?;
        let compute_nodes = extract_compute_nodes(timetable, &timer)?;
        let (compute_nodes, cache_sharing) =
            self.apply_selected_cache_model(&compute_nodes, options.cache_model, &timer);
        let mut widest_path_cache = WidestPathCache::default();
        let node_rooflines =
            self.build_node_rooflines(platform, &compute_nodes, &mut widest_path_cache, &timer)?;
        let pe_summaries =
            self.build_pe_summaries(platform, &node_rooflines, &mut widest_path_cache, &timer)?;
        let scheduled_activities = self.maybe_schedule_activities(
            platform,
            &node_rooflines,
            &mut widest_path_cache,
            options.schedule_activities,
            &timer,
        )?;
        let critical_path = timer.time_result("analyse.critical_path", || {
            critical_path_analysis(&node_rooflines)
        })?;
        let estimates =
            estimate_runtime_bounds(&pe_summaries, scheduled_activities.as_ref(), &critical_path);

        Ok(RooflineReport {
            timetable_stats,
            compute_nodes,
            cache_sharing,
            node_rooflines,
            pe_summaries,
            scheduled_activities,
            critical_path,
            pe_lower_bound_ticks: estimates.pe_lower_bound_ticks,
            estimated_best_case_ticks: estimates.estimated_best_case_ticks,
            scheduled_runtime_ticks: estimates.scheduled_runtime_ticks,
        })
    }

    fn apply_selected_cache_model(
        &self,
        compute_nodes: &[ComputeNodeAnalysis],
        cache_model: CacheModel,
        timer: &AnalysisTimer,
    ) -> (Vec<ComputeNodeAnalysis>, CacheSharingReport) {
        timer.time_value("analyse.cache_model", || {
            apply_cache_model(
                compute_nodes,
                &self.bandwidth_graph,
                &self.platform_cfg,
                cache_model,
            )
        })
    }

    fn build_node_rooflines(
        &self,
        platform: &Platform,
        compute_nodes: &[ComputeNodeAnalysis],
        widest_path_cache: &mut WidestPathCache,
        timer: &AnalysisTimer,
    ) -> RooflineResult<Vec<ComputeNodeRoofline>> {
        timer.time_result("analyse.compute_node_rooflines", || {
            compute_node_rooflines(
                platform,
                compute_nodes,
                &self.bandwidth_graph,
                widest_path_cache,
            )
        })
    }

    fn build_pe_summaries(
        &self,
        platform: &Platform,
        node_rooflines: &[ComputeNodeRoofline],
        widest_path_cache: &mut WidestPathCache,
        timer: &AnalysisTimer,
    ) -> RooflineResult<Vec<PeRooflineSummary>> {
        timer.time_result("analyse.aggregate_pe_rooflines", || {
            aggregate_pe_rooflines(
                platform,
                node_rooflines,
                &self.bandwidth_graph,
                widest_path_cache,
            )
        })
    }

    fn maybe_schedule_activities(
        &self,
        platform: &Platform,
        node_rooflines: &[ComputeNodeRoofline],
        widest_path_cache: &mut WidestPathCache,
        should_schedule: bool,
        timer: &AnalysisTimer,
    ) -> RooflineResult<Option<ScheduledActivities>> {
        if !should_schedule {
            timer.skipped("analyse.schedule_pe_activities");
            return Ok(None);
        }

        timer
            .time_result("analyse.schedule_pe_activities", || {
                schedule_pe_activities(
                    platform,
                    node_rooflines,
                    &self.bandwidth_graph,
                    widest_path_cache,
                )
            })
            .map(Some)
    }
}

fn extract_compute_nodes(
    timetable: &Timetable,
    timer: &AnalysisTimer,
) -> RooflineResult<Vec<ComputeNodeAnalysis>> {
    let compute_nodes = timer.time_result("analyse.extract_compute_nodes", || {
        timetable.compute_node_analyses()
    })?;
    if compute_nodes.is_empty() {
        return Err(SimError("Timetable contains no compute nodes".to_string()).into());
    }
    Ok(compute_nodes)
}

fn estimate_runtime_bounds(
    pe_summaries: &[PeRooflineSummary],
    scheduled_activities: Option<&ScheduledActivities>,
    critical_path: &CriticalPathAnalysis,
) -> RooflineEstimates {
    let pe_lower_bound_ticks = pe_summaries
        .iter()
        .map(|summary| summary.roofline_ticks)
        .fold(0.0, f64::max);
    let estimated_best_case_ticks = critical_path.total_ticks.max(pe_lower_bound_ticks);
    let scheduled_runtime_ticks = scheduled_activities.map(|scheduled_activities| {
        scheduled_activities
            .activities
            .iter()
            .map(|activity| activity.end_ticks)
            .fold(0.0, f64::max)
    });

    RooflineEstimates {
        pe_lower_bound_ticks,
        estimated_best_case_ticks,
        scheduled_runtime_ticks,
    }
}
