// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Engine-free, aggregate roofline estimate for a validated platform and
//! timetable. The estimate accounts task engine service and shared physical
//! traffic resources, then combines those demands in linear time.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use gwr_engine::types::SimError;
use gwr_models::processing_element::compute::{
    ComputeModel, ComputeWork, MatrixWorkShape, TypedWork, WorkStage,
};
use gwr_models::processing_element::program::syntax::{ExprSpec, ReductionOp};
use gwr_models::processing_element::program::validate::{RegionBindings, validate_program};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{HasShape, TensorView};
use gwr_models::roofline::accounting::{Operation, TransactionRun};
use gwr_models::roofline::traffic::ViewTraffic;
use gwr_platform::PreparedPlatform;
use gwr_platform::roofline_resources::ResourceKind;
use gwr_platform::types::PlatformConfig;

use super::{ActivityEstimate, estimate_makespan};
use crate::{AnalysisActivity, Timetable, TimetableGraph};

/// Aggregate result in platform-clock ticks. This model relaxes internal
/// engine barriers and task overlap, so it is intended for relative trends.
#[derive(Clone, Debug, serde::Serialize)]
pub struct RooflineReport {
    pub activities: usize,
    pub estimated_ticks: f64,
    pub pe_local_work_by_pe: BTreeMap<String, f64>,
    pub compute_ticks_by_pe: BTreeMap<String, f64>,
    pub compute_ticks_by_engine: BTreeMap<String, BTreeMap<String, f64>>,
    pub resource_ticks: BTreeMap<String, f64>,
    pub resource_ticks_by_component: BTreeMap<String, f64>,
    pub resource_ticks_by_class: BTreeMap<String, f64>,
    pub bytes_by_memory: BTreeMap<String, u128>,
    pub read_bytes_by_memory: BTreeMap<String, u128>,
    pub written_bytes_by_memory: BTreeMap<String, u128>,
    pub read_bytes_by_pe: BTreeMap<String, u128>,
    pub written_bytes_by_pe: BTreeMap<String, u128>,
    pub approximations: Vec<String>,
}

impl RooflineReport {
    /// Render aggregate and per-component estimates as info-log lines.
    #[must_use]
    pub fn summary_lines(&self) -> Vec<String> {
        let total_compute = self.compute_ticks_by_pe.values().sum::<f64>();
        let total_memory_bytes = self.bytes_by_memory.values().sum::<u128>();
        let total_read_bytes = self.read_bytes_by_memory.values().sum::<u128>();
        let total_written_bytes = self.written_bytes_by_memory.values().sum::<u128>();
        let mut lines = vec![
            "Roofline analysis:".into(),
            format!("  Estimated simulated time: {:.2} ns", self.estimated_ticks),
            format!("  Activities: {}", self.activities),
            format!(
                "  Processing elements: {} | compute work: {total_compute:.2} ticks",
                self.compute_ticks_by_pe.len()
            ),
            format!(
                "  Memory traffic: {total_memory_bytes} bytes ({total_read_bytes} read, {total_written_bytes} written) across {} memories",
                self.bytes_by_memory.len()
            ),
        ];

        if !self.resource_ticks_by_class.is_empty() {
            lines.push("  Resource service demand:".into());
            for (class, ticks) in &self.resource_ticks_by_class {
                lines.push(format!("    {class}: {ticks:.2} resource-ticks"));
            }
        }

        lines.push("Processing elements:".into());
        for (pe, ticks) in &self.compute_ticks_by_pe {
            let read = self.read_bytes_by_pe.get(pe).copied().unwrap_or_default();
            let written = self
                .written_bytes_by_pe
                .get(pe)
                .copied()
                .unwrap_or_default();
            lines.push(format!(
                "  {pe}: compute {ticks:.2} ticks, {read} bytes read, {written} bytes written"
            ));
            if let Some(engines) = self.compute_ticks_by_engine.get(pe) {
                for (engine, engine_ticks) in engines {
                    lines.push(format!("    {engine}: {engine_ticks:.2} ticks"));
                }
            }
        }

        lines.push("Memories:".into());
        for (memory, bytes) in &self.bytes_by_memory {
            let read = self
                .read_bytes_by_memory
                .get(memory)
                .copied()
                .unwrap_or_default();
            let written = self
                .written_bytes_by_memory
                .get(memory)
                .copied()
                .unwrap_or_default();
            lines.push(format!(
                "  {memory}: {bytes} bytes ({read} read, {written} written)"
            ));
        }

        lines.push("Components:".into());
        for (component, ticks) in &self.resource_ticks_by_component {
            lines.push(format!("  {component}: {ticks:.2} resource-ticks"));
        }
        lines
    }
}

/// Fast platform-aware roofline analyzer. It creates no engine events.
pub struct RooflineAnalyzer;

fn compute_signature(activity: &AnalysisActivity) -> Result<Option<String>, SimError> {
    let Some(task) = &activity.task else {
        return Ok(None);
    };
    let program = serde_json::to_string(&task.program)
        .map_err(|error| SimError(format!("serialize program signature: {error}")))?;
    let views = task
        .inputs
        .iter()
        .chain(&task.outputs)
        .map(|view| {
            format!(
                "{:?}:{:?}:{:?}",
                view.dtype(),
                view.tensor().shape().dims(),
                view.shape().dims()
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    Ok(Some(format!("{}|{}|{}", activity.pe_name, program, views)))
}

fn add_byte_count(
    counts: &mut BTreeMap<String, u128>,
    name: String,
    bytes: u128,
) -> Result<(), SimError> {
    let count = counts.entry(name).or_default();
    *count = count
        .checked_add(bytes)
        .ok_or_else(|| SimError("aggregate byte count overflow".into()))?;
    Ok(())
}

fn resource_class(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::PeCompute => "Processing elements",
        ResourceKind::MemoryPayload => "Memories",
        ResourceKind::CacheRequest | ResourceKind::CacheMemoryResponse => "Caches",
        ResourceKind::FabricPlaneDelivery { .. }
        | ResourceKind::FabricPlaneLink { .. }
        | ResourceKind::FabricIngress { .. }
        | ResourceKind::FabricEgress { .. }
        | ResourceKind::FabricLink { .. } => "Fabrics",
    }
}

fn expression_compute_work(
    expr: &ExprSpec,
    region: &gwr_models::processing_element::program::validate::ValidatedRegion,
    result_dtype: DataType,
    count: usize,
) -> Result<(ComputeWork, DataType), SimError> {
    match expr {
        ExprSpec::Read { tensor, .. } => Ok((
            ComputeWork { stages: Vec::new() },
            region
                .value(tensor)
                .ok_or_else(|| SimError(format!("unknown tensor {tensor}")))?
                .dtype,
        )),
        ExprSpec::Call { op, args, .. } => {
            let mut argument_works = Vec::with_capacity(args.len());
            let mut input_dtypes = Vec::with_capacity(args.len());
            for arg in args {
                let (work, dtype) = expression_compute_work(arg, region, result_dtype, count)?;
                argument_works.push(work);
                input_dtypes.push(dtype);
            }
            let max_depth = argument_works
                .iter()
                .map(|work| work.stages.len())
                .max()
                .unwrap_or(0);
            let mut stages = Vec::with_capacity(max_depth + 1);
            for depth in 0..max_depth {
                let operations = argument_works
                    .iter()
                    .filter_map(|work| work.stages.get(depth))
                    .flat_map(|stage| stage.operations.iter().cloned())
                    .collect();
                stages.push(WorkStage { operations });
            }
            stages.push(WorkStage {
                operations: vec![TypedWork::operation(op.clone(), input_dtypes, count)],
            });
            Ok((ComputeWork { stages }, result_dtype))
        }
        ExprSpec::Reduce { .. } | ExprSpec::MatrixMac { .. } => Err(SimError(
            "nested reduce or matrix_mac expression is unsupported by the fast compute estimator"
                .into(),
        )),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "A validated expression tree is lowered in one pass to preserve its engine assignment semantics."
)]
fn estimate_full_region_compute(
    task: &gwr_models::processing_element::task::ProgramTaskConfig,
    config: &gwr_models::processing_element::ProcessingElementConfig,
) -> Result<(u128, BTreeMap<String, u128>), SimError> {
    let region = validate_program(
        &task.program,
        &RegionBindings {
            inputs: task.inputs.clone(),
            outputs: task.outputs.clone(),
        },
    )?;
    let model = ComputeModel::new(&config.compute, &config.operation_classes)?;
    let mut ticks = 0u128;
    let mut engine_work = BTreeMap::<String, u128>::new();
    for &assignment_index in &region.assignment_order {
        let assignment = &region.program.assignments[assignment_index];
        let output_count =
            assignment.axes.iter().try_fold(1usize, |count, axis| {
                count
                    .checked_mul(region.extent(axis).ok_or_else(|| {
                        SimError(format!("assignment axis '{axis}' has no extent"))
                    })?)
                    .ok_or_else(|| SimError("assignment output count overflows".into()))
            })?;
        let result_dtype = region
            .value(&assignment.primary_result())
            .ok_or_else(|| SimError("assignment result has no validated dtype".into()))?
            .dtype;
        let work = match &assignment.expr {
            ExprSpec::Read { .. } => ComputeWork { stages: Vec::new() },
            ExprSpec::Call { .. } => {
                expression_compute_work(&assignment.expr, &region, result_dtype, output_count)?.0
            }
            ExprSpec::Reduce { op, over, value } => {
                let reduction_count = over.iter().try_fold(1usize, |count, axis| {
                    count
                        .checked_mul(region.extent(axis).ok_or_else(|| {
                            SimError(format!("reduction axis '{axis}' has no extent"))
                        })?)
                        .ok_or_else(|| SimError("reduction count overflows".into()))
                })?;
                let elements = output_count
                    .checked_mul(reduction_count)
                    .ok_or_else(|| SimError("reduction element count overflows".into()))?;
                let (mut body, body_dtype) =
                    expression_compute_work(value, &region, result_dtype, elements)?;
                let (op_name, op_count) = match op {
                    ReductionOp::Sum => (
                        "add",
                        output_count
                            .checked_mul(reduction_count.saturating_sub(1))
                            .ok_or_else(|| SimError("reduction add count overflows".into()))?,
                    ),
                    ReductionOp::Min => (
                        "compare",
                        output_count
                            .checked_mul(reduction_count.saturating_sub(1))
                            .ok_or_else(|| SimError("reduction compare count overflows".into()))?,
                    ),
                    ReductionOp::Max => (
                        "maximum",
                        output_count
                            .checked_mul(reduction_count.saturating_sub(1))
                            .ok_or_else(|| SimError("reduction maximum count overflows".into()))?,
                    ),
                };
                if op_count > 0 {
                    body.stages.push(WorkStage {
                        operations: vec![TypedWork::operation(
                            op_name,
                            vec![body_dtype, body_dtype],
                            op_count,
                        )],
                    });
                }
                body
            }
            ExprSpec::MatrixMac {
                args,
                over,
                execution,
                ..
            } => {
                if assignment.axes.len() < 2 || over.len() != 1 || args.len() != 2 {
                    return Err(SimError(
                        "fast matrix_mac estimate requires at least two output axes, one reduction axis, and two operands".into(),
                    ));
                }
                let input_dtypes = args
                    .iter()
                    .map(|arg| match arg {
                        ExprSpec::Read { tensor, .. } => region
                            .value(tensor)
                            .map(|value| value.dtype)
                            .ok_or_else(|| SimError(format!("unknown matrix operand {tensor}"))),
                        _ => Err(SimError(
                            "fast matrix_mac estimate requires direct tensor reads".into(),
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let m_axis = &assignment.axes[assignment.axes.len() - 2];
                let n_axis = &assignment.axes[assignment.axes.len() - 1];
                let extent = |axis: &str| {
                    region
                        .extent(axis)
                        .ok_or_else(|| SimError(format!("matrix axis '{axis}' has no extent")))
                };
                let matrix_work = ComputeWork::matrix_mac(
                    input_dtypes,
                    MatrixWorkShape {
                        m: extent(m_axis)?,
                        n: extent(n_axis)?,
                        k: extent(&over[0])?,
                    },
                )?
                .with_execution(execution.clone());
                let batch_count = assignment.axes[..assignment.axes.len() - 2]
                    .iter()
                    .try_fold(1usize, |count, axis| {
                        count
                            .checked_mul(extent(axis)?)
                            .ok_or_else(|| SimError("matrix batch count overflows".into()))
                    })?;
                let operation = matrix_work.stages[0].operations[0].clone();
                ComputeWork {
                    stages: vec![WorkStage {
                        operations: vec![operation; batch_count],
                    }],
                }
            }
        };
        let plan = model.lower(&work)?;
        let assignment_ticks = u128::from(plan.service_ticks()?);
        ticks = ticks
            .checked_add(assignment_ticks)
            .ok_or_else(|| SimError("full-region compute ticks overflow".into()))?;
        for stage in &plan.stages {
            let mut occupied = BTreeMap::new();
            for batch in stage {
                let (engine, finish, service) = batch
                    .alternatives
                    .iter()
                    .map(|(engine, service)| {
                        occupied
                            .get(engine)
                            .copied()
                            .unwrap_or(0u64)
                            .checked_add(*service)
                            .map(|finish| (*engine, finish, *service))
                            .ok_or_else(|| SimError("compute duration overflows".into()))
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .min_by_key(|(engine, finish, _)| (*finish, *engine))
                    .ok_or_else(|| SimError("compute batch has no engines".into()))?;
                occupied.insert(engine, finish);
                let name = model
                    .engine_names()
                    .get(engine.0)
                    .ok_or_else(|| SimError(format!("unknown compute engine {}", engine.0)))?;
                let demand = engine_work.entry(name.clone()).or_default();
                *demand = demand
                    .checked_add(u128::from(service))
                    .ok_or_else(|| SimError("compute engine work overflows".into()))?;
            }
        }
    }
    Ok((ticks, engine_work))
}

impl RooflineAnalyzer {
    /// Analyze against the same validated configuration used to build the
    /// platform. Striped address maps are handled by analytical traffic
    /// planning, with one route per physical destination.
    pub fn analyze(
        platform: &PlatformConfig,
        timetable: &Timetable,
    ) -> Result<RooflineReport, SimError> {
        Self::analyze_activities(platform, &timetable.analysis_activities()?, false)
    }

    /// Analyze a validated workload graph without constructing an engine or
    /// runtime platform. The platform configuration is independently
    /// validated by `PreparedPlatform` before any estimates are produced.
    pub fn analyze_graph(
        platform: &PlatformConfig,
        graph: &TimetableGraph,
    ) -> Result<RooflineReport, SimError> {
        Self::analyze_activities(platform, &graph.analysis_activities()?, false)
    }

    /// Analyze full task views using ordinary aligned LSU packet traffic.
    /// This calibrated traffic policy retains aggregate overlap, ideal SRAM,
    /// and cold-cache assumptions. It does not predict tiled execution time.
    /// Fragmented views are bounded; striped maps are explicitly unsupported.
    pub fn analyze_graph_lsu_aligned(
        platform: &PlatformConfig,
        graph: &TimetableGraph,
    ) -> Result<RooflineReport, SimError> {
        Self::analyze_activities(platform, &graph.analysis_activities()?, true)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "This pass joins task estimates, route demands, and shared resource makespan accounting."
    )]
    fn analyze_activities(
        platform: &PlatformConfig,
        snapshots: &[AnalysisActivity],
        aligned_lsu: bool,
    ) -> Result<RooflineReport, SimError> {
        let started = Instant::now();
        let prepared = PreparedPlatform::new(platform)?;
        let prepared_elapsed = started.elapsed();
        let timings = std::env::var_os("GWR_ROOFLINE_TIMINGS").is_some();
        if timings {
            eprintln!(
                "roofline timing: prepared={:.3}s",
                prepared_elapsed.as_secs_f64()
            );
        }
        let routes = prepared.roofline_routes()?;
        let routes_elapsed = started.elapsed().saturating_sub(prepared_elapsed);
        if timings {
            eprintln!(
                "roofline timing: routes={:.3}s",
                routes_elapsed.as_secs_f64()
            );
        }
        let resources = routes.resources();
        let capacities = resources.capacities();
        if timings {
            eprintln!("roofline timing: activities={}", snapshots.len());
        }
        if snapshots.is_empty() {
            return Err(SimError(
                "Timetable contains no compute or transfer activities".into(),
            ));
        }
        let mut memory_names = HashMap::new();
        for memory in platform.memories.iter().flatten() {
            memory_names.insert(prepared.device_id(&memory.name)?, memory.name.as_str());
        }

        let mut pe_local_work = BTreeMap::<String, f64>::new();
        let mut compute_work = BTreeMap::<String, f64>::new();
        let mut compute_by_engine = BTreeMap::<String, BTreeMap<String, f64>>::new();
        let mut memory_bytes = BTreeMap::<String, u128>::new();
        let mut read_bytes_by_memory = BTreeMap::<String, u128>::new();
        let mut written_bytes_by_memory = BTreeMap::<String, u128>::new();
        let mut read_bytes_by_pe = BTreeMap::<String, u128>::new();
        let mut written_bytes_by_pe = BTreeMap::<String, u128>::new();
        for pe in platform.processing_elements.iter().flatten() {
            pe_local_work.insert(pe.name.clone(), 0.0);
            compute_work.insert(pe.name.clone(), 0.0);
            read_bytes_by_pe.insert(pe.name.clone(), 0);
            written_bytes_by_pe.insert(pe.name.clone(), 0);
        }
        for memory in platform.memories.iter().flatten() {
            memory_bytes.insert(memory.name.clone(), 0);
            read_bytes_by_memory.insert(memory.name.clone(), 0);
            written_bytes_by_memory.insert(memory.name.clone(), 0);
        }
        let mut activities = Vec::with_capacity(snapshots.len());
        let mut task_planning_elapsed = std::time::Duration::ZERO;
        let mut traffic_planning_elapsed = std::time::Duration::ZERO;
        let mut compute_tick_cache = HashMap::<String, (u128, BTreeMap<String, u128>)>::new();
        let mut routed_runs =
            HashMap::<(String, String, bool), BTreeMap<(u128, u128, u128), u128>>::new();
        let mut progress_started = Instant::now();
        let activity_positions: HashMap<_, _> = snapshots
            .iter()
            .enumerate()
            .map(|(position, activity)| (activity.node_index, position))
            .collect();
        for (activity_index, activity) in snapshots.iter().enumerate() {
            let pe_section = platform
                .processing_elements
                .as_ref()
                .and_then(|pes| pes.iter().find(|pe| pe.name == activity.pe_name))
                .ok_or_else(|| SimError(format!("Missing platform PE '{}'", activity.pe_name)))?;
            let pe_config = pe_section
                .effective_config(&platform.operation_classes, &platform.compute_profiles)?;
            let task_started = Instant::now();
            let signature = compute_signature(activity)?;
            let (compute_ticks, engine_work) =
                if let (Some(task), Some(signature)) = (&activity.task, signature) {
                    if let Some(ticks) = compute_tick_cache.get(&signature) {
                        ticks.clone()
                    } else {
                        let ticks = estimate_full_region_compute(task, &pe_config)?;
                        compute_tick_cache.insert(signature.clone(), ticks.clone());
                        ticks
                    }
                } else {
                    (0, BTreeMap::new())
                };
            task_planning_elapsed += task_started.elapsed();
            if timings && activity_index < 10 {
                eprintln!(
                    "roofline timing: compute_estimate_index={} elapsed={:.3}s unique_compute_signatures={}",
                    activity_index,
                    task_started.elapsed().as_secs_f64(),
                    compute_tick_cache.len()
                );
            }
            let memory_map = prepared.memory_map(&pe_section.memory_map)?;
            let views: Vec<(&TensorView, Operation)> = if let Some(task) = &activity.task {
                task.inputs
                    .iter()
                    .map(|view| (view, Operation::Read))
                    .chain(
                        task.outputs
                            .iter()
                            .map(|view| (view, Operation::WriteNonPosted)),
                    )
                    .collect()
            } else {
                let (source, destination) = activity.transfer_views.as_ref().ok_or_else(|| {
                    SimError(format!(
                        "Task '{}' has no compute or transfer views",
                        activity.id
                    ))
                })?;
                vec![
                    (source, Operation::Read),
                    (destination, Operation::WriteNonPosted),
                ]
            };
            let mut activity_memory_bytes = BTreeMap::<String, u128>::new();
            for (view, operation) in views {
                let traffic_started = Instant::now();
                let plan_traffic = if aligned_lsu {
                    ViewTraffic::new_lsu_aligned
                } else {
                    ViewTraffic::new_analytical
                };
                let traffic = plan_traffic(
                    view,
                    operation,
                    memory_map,
                    pe_config.lsu_access_bytes.max(1) as u128,
                    pe_config.overhead_size_bytes as u128,
                )?;
                for destination in traffic.destinations() {
                    let memory = memory_names.get(&destination.device()).ok_or_else(|| {
                        SimError(format!("Unknown memory device {:?}", destination.device()))
                    })?;
                    add_byte_count(
                        &mut activity_memory_bytes,
                        (*memory).to_owned(),
                        destination.traffic().payload,
                    )?;
                    add_byte_count(
                        &mut memory_bytes,
                        (*memory).to_owned(),
                        destination.traffic().payload,
                    )?;
                    let pe_bytes = if operation == Operation::Read {
                        &mut read_bytes_by_pe
                    } else {
                        &mut written_bytes_by_pe
                    };
                    add_byte_count(
                        pe_bytes,
                        activity.pe_name.clone(),
                        destination.traffic().payload,
                    )?;
                    let memory_bytes_for_direction = if operation == Operation::Read {
                        &mut read_bytes_by_memory
                    } else {
                        &mut written_bytes_by_memory
                    };
                    add_byte_count(
                        memory_bytes_for_direction,
                        (*memory).to_owned(),
                        destination.traffic().payload,
                    )?;
                    for run in destination.runs() {
                        let count = routed_runs
                            .entry((
                                activity.pe_name.clone(),
                                (*memory).to_owned(),
                                operation == Operation::Read,
                            ))
                            .or_default()
                            .entry((
                                run.payload_bytes,
                                run.request_header_bytes,
                                run.response_header_bytes,
                            ))
                            .or_default();
                        *count = count.checked_add(run.count).ok_or_else(|| {
                            SimError("aggregate transaction count overflow".into())
                        })?;
                    }
                }
                traffic_planning_elapsed += traffic_started.elapsed();
            }
            // Dependency paths must include each task's own memory service,
            // even when successive tasks use independent memory resources.
            // Global aggregate demand alone loses this serialization floor.
            let memory_floor =
                activity_memory_bytes
                    .iter()
                    .try_fold(0.0f64, |floor, (memory, bytes)| {
                        let resource = resources
                            .index(memory, ResourceKind::MemoryPayload)
                            .ok_or_else(|| {
                                SimError(format!("Missing memory resource '{memory}'"))
                            })?;
                        Ok::<_, SimError>(floor.max(*bytes as f64 / capacities[resource]))
                    })?;
            *pe_local_work.entry(activity.pe_name.clone()).or_default() += compute_ticks as f64;
            *compute_work.entry(activity.pe_name.clone()).or_default() += compute_ticks as f64;
            let pe_engine_work = compute_by_engine
                .entry(activity.pe_name.clone())
                .or_default();
            for (engine, ticks) in &engine_work {
                *pe_engine_work.entry(engine.clone()).or_default() += *ticks as f64;
            }
            let engine_demands = engine_work
                .into_iter()
                .map(|(engine, ticks)| (format!("{}:{engine}", activity.pe_name), ticks as f64))
                .collect();
            activities.push(ActivityEstimate {
                pe: activity.pe_name.clone(),
                local_ticks: (compute_ticks as f64).max(memory_floor),
                engine_demands,
                predecessors: activity
                    .predecessors
                    .iter()
                    .map(|predecessor| {
                        activity_positions.get(predecessor).copied().ok_or_else(|| {
                            SimError(format!("Missing compute predecessor {predecessor}"))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                resources: BTreeMap::new(),
            });
            if timings && (activity_index + 1) % 100 == 0 {
                eprintln!(
                    "roofline timing: analyzed_activities={} last_100_elapsed={:.3}s task_total={:.3}s traffic_route_total={:.3}s",
                    activity_index + 1,
                    progress_started.elapsed().as_secs_f64(),
                    task_planning_elapsed.as_secs_f64(),
                    traffic_planning_elapsed.as_secs_f64()
                );
                progress_started = Instant::now();
            }
        }

        let route_started = Instant::now();
        let mut resource_work = BTreeMap::<String, f64>::new();
        let mut resource_work_by_component = BTreeMap::<String, f64>::new();
        let mut resource_work_by_class = BTreeMap::from([
            ("PE interfaces".to_owned(), 0.0),
            ("Caches".to_owned(), 0.0),
            ("Memories".to_owned(), 0.0),
            ("Fabrics".to_owned(), 0.0),
        ]);
        for resource in resources.resources() {
            if resource.kind != ResourceKind::PeCompute {
                resource_work_by_component
                    .entry(resource.component.clone())
                    .or_default();
            }
        }
        for ((pe, memory, is_read), runs) in routed_runs {
            let operation = if is_read {
                Operation::Read
            } else {
                Operation::WriteNonPosted
            };
            let runs = runs
                .into_iter()
                .map(
                    |((payload_bytes, request_header_bytes, response_header_bytes), count)| {
                        TransactionRun {
                            payload_bytes,
                            count,
                            request_header_bytes,
                            response_header_bytes,
                        }
                    },
                )
                .collect::<Vec<_>>();
            let service = routes.plan_uniform_all_miss(&pe, &memory, operation, &runs)?;
            for (resource_index, bytes) in &service.resource_demands {
                let descriptor = resources.resources().get(*resource_index).ok_or_else(|| {
                    SimError(format!("Unknown roofline resource {resource_index}"))
                })?;
                let key = format!("{}:{:?}", descriptor.component, descriptor.kind);
                let service_ticks = *bytes / capacities[*resource_index];
                *resource_work.entry(key).or_default() += service_ticks;
                *resource_work_by_component
                    .entry(descriptor.component.clone())
                    .or_default() += service_ticks;
                let class = resource_class(descriptor.kind);
                *resource_work_by_class.entry(class.into()).or_default() += service_ticks;
            }
        }
        traffic_planning_elapsed += route_started.elapsed();
        activities.push(ActivityEstimate {
            pe: "__roofline_global_resources__".into(),
            local_ticks: 0.0,
            engine_demands: BTreeMap::new(),
            predecessors: Vec::new(),
            resources: resource_work.clone(),
        });

        let estimated_ticks = estimate_makespan(&activities)?;
        if timings {
            eprintln!(
                "roofline timing: prepared={:.3}s routes={:.3}s tasks={:.3}s traffic+routes={:.3}s total={:.3}s",
                prepared_elapsed.as_secs_f64(),
                routes_elapsed.as_secs_f64(),
                task_planning_elapsed.as_secs_f64(),
                traffic_planning_elapsed.as_secs_f64(),
                started.elapsed().as_secs_f64()
            );
            eprintln!(
                "roofline timing: unique_compute_signatures={} activities={}",
                compute_tick_cache.len(),
                snapshots.len()
            );
        }
        Ok(RooflineReport {
            activities: snapshots.len(),
            estimated_ticks,
            pe_local_work_by_pe: pe_local_work,
            compute_ticks_by_pe: compute_work,
            compute_ticks_by_engine: compute_by_engine,
            resource_ticks: resource_work,
            resource_ticks_by_component: resource_work_by_component,
            resource_ticks_by_class: resource_work_by_class,
            bytes_by_memory: memory_bytes,
            read_bytes_by_memory,
            written_bytes_by_memory,
            read_bytes_by_pe,
            written_bytes_by_pe,
            approximations: vec![
                "Compute work is estimated from full-region typed operation counts; explicit tiling and pipeline effects are omitted.".into(),
                if aligned_lsu {
                    "Memory traffic uses aligned full LSU packets over whole task views; tile-induced rereads and SRAM effects are omitted.".into()
                } else {
                    "Memory traffic uses whole logical task views, so tile-induced rereads and SRAM effects are omitted.".into()
                },
                "Dependency paths retain per-task memory service floors. Global resource demand is combined as an aggregate bandwidth floor using averaged route selection; all cache reads are modeled as misses.".into(),
                "The analyzer uses physical platform configuration directly; no event simulation or model selection is involved.".into(),
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::RooflineReport;

    #[test]
    fn summary_lines_include_system_totals_and_per_component_details() {
        let report = RooflineReport {
            activities: 3,
            estimated_ticks: 12.3456,
            pe_local_work_by_pe: BTreeMap::from([("pe0".into(), 8.0)]),
            compute_ticks_by_pe: BTreeMap::from([("pe0".into(), 8.0)]),
            compute_ticks_by_engine: BTreeMap::from([(
                "pe0".into(),
                BTreeMap::from([("vector".into(), 8.0)]),
            )]),
            resource_ticks: BTreeMap::new(),
            resource_ticks_by_component: BTreeMap::from([
                ("fabric0".into(), 2.125),
                ("mem0".into(), 4.25),
            ]),
            resource_ticks_by_class: BTreeMap::from([
                ("Fabrics".into(), 2.125),
                ("Memory".into(), 4.25),
            ]),
            bytes_by_memory: BTreeMap::from([("mem0".into(), 128)]),
            read_bytes_by_memory: BTreeMap::from([("mem0".into(), 96)]),
            written_bytes_by_memory: BTreeMap::from([("mem0".into(), 32)]),
            read_bytes_by_pe: BTreeMap::from([("pe0".into(), 96)]),
            written_bytes_by_pe: BTreeMap::from([("pe0".into(), 32)]),
            approximations: Vec::new(),
        };

        let summary = report.summary_lines().join("\n");
        assert!(summary.contains("Estimated simulated time: 12.35 ns"));
        assert!(summary.contains("compute work: 8.00 ticks"));
        assert!(summary.contains("Memory traffic: 128 bytes (96 read, 32 written)"));
        assert!(summary.contains("Fabrics: 2.12 resource-ticks"));
        assert!(summary.contains("pe0: compute 8.00 ticks, 96 bytes read, 32 bytes written"));
        assert!(summary.contains("vector: 8.00 ticks"));
        assert!(summary.contains("mem0: 128 bytes (96 read, 32 written)"));
        assert!(summary.contains("fabric0: 2.12 resource-ticks"));
    }
}
