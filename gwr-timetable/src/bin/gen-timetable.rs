// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Generate random ML-like Timetable YAML for a Platform.
//!
//! The generator starts from a single initial tensor with a random shape and
//! then grows a timetable in both directions. Backward expansion creates a
//! producer for an existing tensor. Forward expansion creates a consumer of an
//! existing tensor.
//!
//! `Frontier` owns the current set of possible expansion points.
//!
//! Backward layers are given negative layer indices. Forward layers are given
//! positive layer indices. The `--depth` limits the total span of generated
//! layers around the initial tensor.
//!
//! Program templates are selected using the user-defined weights.
//! Once a template is chosen, the generator builds input and output tensor
//! definitions and partitions each operation over an outer output axis. Each
//! partition has input and output views and is assigned to a processing
//! element. The new tensors are added as possible expansion points. The
//! generator iterates until the compute-node budget is exhausted.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{read_to_string, write};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::{fmt, io};

use byte_unit::{Byte, UnitType};
use clap::Parser;
use gwr_engine::engine::Engine;
use gwr_engine::types::SimError;
use gwr_models::processing_element::ProcessingElement;
use gwr_models::processing_element::compute::ComputeWork;
use gwr_models::processing_element::program::PreparedProgram;
use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
use gwr_models::processing_element::program::validate::RegionBindings;
use gwr_models::processing_element::task::PreparedTask;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{HasShape, Shape, Tensor, TensorView};
use gwr_platform::Platform;
use gwr_platform::types::PlatformConfig;
use gwr_timetable::timetable_file::{
    EdgeKind, EdgeSection, NodeSection, TensorConfigSection, TensorViewSection, TimetableFile,
};
use gwr_track::tracker::{DevNullTracker, Tracker};
use log::{LevelFilter, Log, Metadata, Record, debug, info, max_level, set_logger, set_max_level};
use rand::prelude::{RngExt, SeedableRng};
use rand::rngs::StdRng;
use yaml_serde::{from_str, to_string};

#[path = "gen-timetable/programs.rs"]
mod programs;

#[path = "gen-timetable/templates.rs"]
mod templates;
use templates::{ExpansionDirection, ProgramTemplate};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() -> Result<()> {
    let args = Cli::parse();
    init_logging(args.debug);

    let platform_yaml = read_to_string(&args.platform)?;
    let platform: PlatformConfig = from_str(&platform_yaml).map_err(|e| {
        error_from_str(format!(
            "failed to parse platform YAML using gwr_platform::types::PlatformConfig: {e}"
        ))
    })?;

    let out_path = args.out.clone();
    let generator = Generator::new(args, &platform)?;
    let timetable = generate(generator)?;
    let yaml = to_string(&timetable)
        .map_err(|e| error_from_str(format!("failed to serialise timetable YAML: {e}")))?;
    write(&out_path, yaml)
        .map_err(|e| error_from_str(format!("failed to write {}: {e}", out_path.display())))?;
    info!("Wrote graph to {}", out_path.display());
    Ok(())
}

static LOGGER: SimpleLogger = SimpleLogger;

fn init_logging(debug_enabled: bool) {
    let level = if debug_enabled {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };

    let _ = set_logger(&LOGGER);
    set_max_level(level);
}

struct SimpleLogger;

impl Log for SimpleLogger {
    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{}", record.args());
        }
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= max_level()
    }

    fn flush(&self) {}
}

fn generate(mut generator: Generator) -> Result<TimetableFile> {
    let shape = generator.random_shape()?;
    let init_dtype = generator.args.init_dtype;
    let seed_tensor = generator.make_tensor_with_dtype("initial", &shape, &init_dtype)?;
    generator.expansion_points.try_to_add_expansion_point(
        seed_tensor.clone(),
        0,
        ExpansionDirection::Backward,
    );
    generator.expansion_points.try_to_add_expansion_point(
        seed_tensor,
        0,
        ExpansionDirection::Forward,
    );

    while generator.remaining_compute_nodes > 0 {
        let Some((idx, template)) = generator.choose_expansion()? else {
            break;
        };
        let point = generator.expansion_points.swap_remove(idx);
        generator.expand_point(&point, &template)?;
    }

    if generator.template_counts.iter().all(|count| *count == 0) {
        return Err(error_from_str(
            "no supported compute templates for the platform and initial tensor dtype",
        ));
    }

    generator.print_summary()?;
    for tensor in &generator.tensors {
        generator.nodes.push(NodeSection::Tensor {
            id: tensor_id(tensor).to_string(),
            config: TensorConfigSection {
                addr: tensor.addr(),
                dtype: *tensor.dtype(),
                shape: tensor.shape().dims().to_vec(),
            },
        });
    }

    let timetable = TimetableFile {
        nodes: generator.nodes,
        edges: generator.edges,
    };
    timetable.validate().map_err(boxed_error)?;
    Ok(timetable)
}

struct Generator {
    args: Cli,
    rng: StdRng,
    _engine: Engine,
    _platform: Platform,
    pes: Vec<GeneratorPe>,
    physical: Vec<Range<u128>>,
    selected: Vec<SelectedCompute>,
    tensors: Vec<Tensor>,
    // Successfully allocated tensors are a prefix; registration appends new
    // ones.
    allocated_tensors: usize,
    nodes: Vec<NodeSection>,
    edges: Vec<EdgeSection>,
    next_node_idx: usize,
    template_counts: [usize; NUM_TEMPLATES],
    expansion_points: Frontier,
    remaining_compute_nodes: usize,
    last_pe_idx: usize,
}

impl Generator {
    fn new(args: Cli, platform: &PlatformConfig) -> Result<Self> {
        validate_args(&args)?;

        let tracker: Tracker = Rc::new(DevNullTracker);
        let mut engine = Engine::new(&tracker);
        let clock = engine.default_clock();
        let runtime = Platform::from_config(&engine, &clock, platform)?;
        let pes = platform
            .processing_elements
            .iter()
            .flatten()
            .map(|section| {
                let model = runtime.pe(&section.name)?.clone();
                let reachable = runtime.reachable_memories(&section.name)?;
                let memory_ranges = coalesce(
                    model
                        .memory_map()
                        .regions()
                        .filter(|region| reachable.contains(&region.device))
                        .map(|region| u128::from(region.start)..u128::from(region.end) + 1)
                        .collect(),
                );
                Ok(GeneratorPe {
                    name: section.name.clone(),
                    model,
                    memory_ranges,
                })
            })
            .collect::<std::result::Result<Vec<_>, SimError>>()?;
        if pes.is_empty() {
            return Err(error_from_str("platform contains zero processing elements"));
        }
        let physical = coalesce(
            platform
                .memories
                .iter()
                .flatten()
                .map(|section| {
                    let memory = runtime.memory(&section.name).expect("resolved memory");
                    u128::from(memory.base_address())
                        ..u128::from(memory.base_address()) + memory.capacity_bytes() as u128
                })
                .collect(),
        );
        if physical.is_empty() {
            return Err(error_from_str("platform contains no memories"));
        }

        let seed = args.seed;
        let target_depth = args.depth;
        let remaining_compute_nodes = args.max_compute_nodes;
        Ok(Self {
            args,
            rng: StdRng::seed_from_u64(seed),
            _engine: engine,
            _platform: runtime,
            pes,
            physical,
            selected: Vec::new(),
            tensors: Vec::new(),
            allocated_tensors: 0,
            nodes: Vec::new(),
            edges: Vec::new(),
            next_node_idx: 0,
            template_counts: [0; NUM_TEMPLATES],
            expansion_points: Frontier::new(target_depth),
            remaining_compute_nodes,
            last_pe_idx: 0,
        })
    }

    fn print_summary(&self) -> Result<()> {
        info!(
            "Created {} tensors / {} nodes / {} edges",
            self.tensors.len(),
            self.nodes.len(),
            self.edges.len()
        );
        let [add_template, gemm_template, reduce_template] = program_templates();
        info!(
            "Program templates: add={} gemm={} reduce_max={}",
            self.template_counts[template_index(&add_template)],
            self.template_counts[template_index(&gemm_template)],
            self.template_counts[template_index(&reduce_template)]
        );
        let total_bytes = self.tensors.iter().try_fold(0u64, |total, tensor| {
            let num_bytes = u64::try_from(tensor.num_bytes())?;
            total
                .checked_add(num_bytes)
                .ok_or_else(|| error_from_str("total tensor storage size overflows"))
        })?;
        info!(
            "Tensors use {total_bytes} bytes ({:.2})",
            Byte::from_u64(total_bytes).get_appropriate_unit(UnitType::Binary)
        );
        Ok(())
    }

    fn expand_point(&mut self, point: &ExpansionPoint, template: &ProgramTemplate) -> Result<()> {
        let next_layer = point.next_layer();

        let compute_id = self.next_id(next_layer, template_name(template));

        debug!("\nExpand {point}: {}", point.tensor.shape());
        debug!("  Compute: {compute_id} (layer {next_layer}):");

        let (input_tensors, output_tensors) = match point.direction {
            ExpansionDirection::Backward => {
                self.create_backward_io(&compute_id, template, point)?
            }
            ExpansionDirection::Forward => self.create_forward_io(&compute_id, template, point)?,
        };

        for (idx, input_tensor) in input_tensors.iter().enumerate() {
            if let Some(input_tensor) = input_tensor {
                debug!("  Input{idx}: {}", input_tensor.shape());
            } else {
                debug!("  Input{idx}: None");
            }
        }

        for (idx, output_tensor) in output_tensors.iter().enumerate() {
            if let Some(output_tensor) = output_tensor {
                debug!("  Output{idx}: {}", output_tensor.shape());
            } else {
                debug!("  Output{idx}: None");
            }
        }

        let boundary = build_full_tensor_boundary(&input_tensors, &output_tensors);

        let created = self.create_partitioned_compute(
            &compute_id,
            template,
            &boundary,
            &input_tensors,
            &output_tensors,
        )?;

        self.remaining_compute_nodes -= created;
        self.template_counts[template_index(template)] += created;
        self.expansion_points.record_expanded_point(point);
        self.enqueue_followups(point, &input_tensors, &output_tensors);

        Ok(())
    }

    fn create_backward_io(
        &mut self,
        compute_id: &str,
        template: &ProgramTemplate,
        point: &ExpansionPoint,
    ) -> Result<TemplateIo> {
        let mut output_tensors = vec![Some(point.tensor.clone())];
        if matches!(template, ProgramTemplate::ReduceMax) {
            template
                .add_optional_outputs(&mut output_tensors, self.args.expand_ratio, &mut self.rng)
                .map_err(boxed_error)?;
        }
        self.add_ids_and_register(&format!("{compute_id}_output"), &mut output_tensors)?;

        let mut input_tensors = self.create_inputs_for_template(template, &output_tensors)?;
        self.add_ids_and_register(&format!("{compute_id}_input"), &mut input_tensors)?;

        Ok((input_tensors, output_tensors))
    }

    fn create_forward_io(
        &mut self,
        compute_id: &str,
        template: &ProgramTemplate,
        point: &ExpansionPoint,
    ) -> Result<TemplateIo> {
        let mut input_tensors = self.create_forward_inputs(template, &point.tensor)?;
        self.add_ids_and_register(&format!("{compute_id}_input"), &mut input_tensors)?;
        let mut output_tensors = self.create_outputs_for_template(template, &input_tensors)?;
        self.add_ids_and_register(&format!("{compute_id}_output"), &mut output_tensors)?;

        Ok((input_tensors, output_tensors))
    }

    fn create_forward_inputs(
        &mut self,
        template: &ProgramTemplate,
        input_tensor: &Tensor,
    ) -> Result<Vec<Option<Tensor>>> {
        match template {
            ProgramTemplate::Add => {
                let other = Tensor::new(input_tensor.shape().dims(), input_tensor.dtype(), 0)
                    .map_err(boxed_error)?;
                let inputs = vec![Some(input_tensor.clone()), Some(other)];
                Ok(inputs)
            }
            ProgramTemplate::Gemm => {
                let other_shape =
                    ProgramTemplate::gemm_rhs_shape(input_tensor).map_err(boxed_error)?;
                let other = Tensor::from_shape(other_shape, input_tensor.dtype(), 0)
                    .map_err(boxed_error)?;
                let mut inputs = vec![Some(input_tensor.clone()), Some(other)];
                let ratio = self.optional_input_ratio(template, *input_tensor.dtype());
                template
                    .add_optional_inputs(&mut inputs, ratio, &mut self.rng)
                    .map_err(boxed_error)?;
                Ok(inputs)
            }
            ProgramTemplate::ReduceMax => Ok(vec![Some(input_tensor.clone())]),
        }
    }

    fn enqueue_followups(
        &mut self,
        point: &ExpansionPoint,
        input_tensors: &[Option<Tensor>],
        output_tensors: &[Option<Tensor>],
    ) {
        let next_layer = point.next_layer();
        let input_tensor_layer = match point.direction {
            ExpansionDirection::Backward => next_layer,
            ExpansionDirection::Forward => point.tensor_layer,
        };
        let output_tensor_layer = match point.direction {
            ExpansionDirection::Backward => point.tensor_layer,
            ExpansionDirection::Forward => next_layer,
        };

        for input_tensor in input_tensors.iter().flatten() {
            self.expansion_points.record_consumer(input_tensor);
            if tensor_id(input_tensor) == tensor_id(&point.tensor) {
                continue;
            }
            self.expansion_points.try_to_add_expansion_point(
                input_tensor.clone(),
                input_tensor_layer,
                ExpansionDirection::Backward,
            );
        }

        for output_tensor in output_tensors.iter().flatten() {
            self.expansion_points.record_producer(output_tensor);
            if tensor_id(output_tensor) == tensor_id(&point.tensor) {
                continue;
            }
            self.expansion_points.try_to_add_expansion_point(
                output_tensor.clone(),
                output_tensor_layer,
                ExpansionDirection::Forward,
            );
        }
    }

    fn create_partitioned_compute(
        &mut self,
        compute_id: &str,
        template: &ProgramTemplate,
        boundary: &GenerationBoundary,
        input_tensors: &[Option<Tensor>],
        output_tensors: &[Option<Tensor>],
    ) -> Result<usize> {
        // Each template exposes an outer output axis that can be divided into
        // independent regions. GEMM uses m so the right operand remains shared.
        let output = boundary.outputs[0].as_ref().expect("generated output");
        let (axis, output_dim, input_dims): (&str, usize, &[Option<usize>]) = match template {
            ProgramTemplate::Add => ("d0", 0, &[Some(0), Some(0)]),
            ProgramTemplate::Gemm => {
                let m = output.num_dims() - 2;
                ("m", m, &[Some(m), None, None])
            }
            ProgramTemplate::ReduceMax => ("d0", 0, &[Some(0)]),
        };
        let extent = output.shape().dims()[output_dim];
        let eligible = self.eligible_pe_indices(
            template,
            *input_tensors[0].as_ref().expect("generated input").dtype(),
            matches!(template, ProgramTemplate::Gemm) && input_tensors.len() == 3,
        );
        if eligible.is_empty() {
            return Err(error_from_str(format!(
                "no PE supports the generated {} program",
                template_name(template)
            )));
        }
        let count = self.partition_count(extent, eligible.len());
        let base = extent / count;
        let extra = extent % count;
        let mut offset = 0;
        for part in 0..count {
            let length = base + usize::from(part < extra);
            let part_boundary = boundary.partition(input_dims, output_dim, offset, length)?;
            let part_id = if count == 1 {
                compute_id.to_string()
            } else {
                format!("{compute_id}_part{part}")
            };
            let plan = self.plan_partition(&part_id, template, &part_boundary, axis, &eligible)?;
            self.commit_placement(&part_id, part_boundary, input_tensors, output_tensors, plan);
            offset += length;
        }
        Ok(count)
    }

    fn plan_partition(
        &self,
        id: &str,
        template: &ProgramTemplate,
        boundary: &GenerationBoundary,
        axis: &str,
        eligible: &[usize],
    ) -> Result<PlacementPlan> {
        let mut failures = Vec::new();
        for candidate in self.compute_candidates(template, boundary, eligible) {
            match self.try_placement(id, template, boundary, axis, candidate) {
                Ok(plan) => return Ok(plan),
                Err(error) => failures.push(format!("{}: {error}", self.pes[candidate.0].name)),
            }
        }
        Err(error_from_str(format!(
            "no feasible placement found for this generated mapping ({id}): {}",
            failures.join("; ")
        )))
    }

    fn try_placement(
        &self,
        id: &str,
        template: &ProgramTemplate,
        boundary: &GenerationBoundary,
        axis: &str,
        (pe_idx, accumulator): (usize, DataType),
    ) -> Result<PlacementPlan> {
        let program = build_program_for_boundary(template, boundary, accumulator)?;
        let tensors = self.allocate_tensors(pe_idx, boundary)?;
        let relocated = relocate_boundary(boundary, &tensors)?;
        let (tiling, prepared) =
            find_tiling_for_pe(id, &program, &relocated, axis, &self.pes[pe_idx].model)?;
        let affected_tasks = self.prepare_affected(&tensors)?;
        Ok(PlacementPlan {
            pe_idx,
            program,
            tensors,
            tiling,
            prepared,
            affected_tasks,
        })
    }

    fn commit_placement(
        &mut self,
        id: &str,
        boundary: GenerationBoundary,
        input_tensors: &[Option<Tensor>],
        output_tensors: &[Option<Tensor>],
        plan: PlacementPlan,
    ) {
        let PlacementPlan {
            pe_idx,
            program,
            tensors,
            tiling,
            prepared,
            affected_tasks,
        } = plan;
        // All allocation, binding and preparation can fail only during the
        // immutable attempt. Installation below has no fallible steps.
        self.tensors = tensors;
        self.allocated_tensors = self.tensors.len();
        for (index, prepared) in affected_tasks {
            self.selected[index].prepared = prepared;
        }
        let node = self.nodes.len();
        self.create_compute(
            id,
            &boundary,
            input_tensors,
            output_tensors,
            ComputeMapping {
                pe_idx,
                program,
                tiling,
            },
        );
        self.selected.push(SelectedCompute {
            node,
            pe: pe_idx,
            boundary,
            prepared,
        });
        self.last_pe_idx = (pe_idx + 1) % self.pes.len();
    }

    fn compute_candidates(
        &self,
        template: &ProgramTemplate,
        boundary: &GenerationBoundary,
        eligible: &[usize],
    ) -> Vec<(usize, DataType)> {
        let output_dtype = *boundary.outputs[0]
            .as_ref()
            .expect("generated output")
            .dtype();
        let mut candidates = eligible.to_vec();
        candidates
            .sort_by_key(|index| (index + self.pes.len() - self.last_pe_idx) % self.pes.len());
        candidates
            .into_iter()
            .flat_map(|pe_idx| {
                let mut accumulators = if matches!(template, ProgramTemplate::Gemm) {
                    let inputs = [&boundary.inputs[0], &boundary.inputs[1]]
                        .map(|view| *view.as_ref().expect("generated matrix operand").dtype());
                    self.pes[pe_idx]
                        .model
                        .compute_model()
                        .matrix_accumulators(inputs)
                } else {
                    vec![output_dtype]
                };
                // Prefer destination precision when feasible, then try other
                // declared precisions deterministically.
                accumulators.sort_by_key(|dtype| (*dtype != output_dtype, *dtype));
                accumulators.into_iter().map(move |dtype| (pe_idx, dtype))
            })
            .collect()
    }

    fn partition_count(&self, extent: usize, eligible_count: usize) -> usize {
        let requested_size = self
            .args
            .partition_size
            .unwrap_or_else(|| extent.div_ceil(eligible_count));
        extent
            .div_ceil(requested_size)
            .min(self.remaining_compute_nodes)
    }

    fn prepare_affected(&self, tensors: &[Tensor]) -> Result<Vec<(usize, Rc<PreparedTask>)>> {
        let current: BTreeMap<_, _> = self.tensors.iter().map(|t| (tensor_id(t), t)).collect();
        let changed: BTreeSet<_> = tensors
            .iter()
            .filter_map(|proposed| {
                let previous = current[tensor_id(proposed)];
                (previous.addr() != proposed.addr()
                    || previous.dtype() != proposed.dtype()
                    || previous.shape() != proposed.shape())
                .then_some(tensor_id(proposed))
            })
            .collect();
        self.selected
            .iter()
            .enumerate()
            .filter(|(_, selected)| {
                selected
                    .boundary
                    .inputs
                    .iter()
                    .chain(&selected.boundary.outputs)
                    .flatten()
                    .any(|view| changed.contains(tensor_id(view.tensor())))
            })
            .map(|(index, selected)| {
                // Selection fixes the PE instance, serialized program, tiling,
                // and view shapes/offsets for the rest of generation. Only
                // tensor storage can change. Compare authoritative locations
                // above, not the original boundary's unrelocated addresses.
                let NodeSection::Compute {
                    id,
                    program,
                    tiling,
                    ..
                } = &self.nodes[selected.node]
                else {
                    unreachable!()
                };
                let prepared = prepare_boundary(
                    &self.pes[selected.pe].model,
                    id,
                    program,
                    tiling.as_ref(),
                    &relocate_boundary(&selected.boundary, tensors)?,
                )?;
                Ok((index, prepared))
            })
            .collect()
    }

    fn create_compute(
        &mut self,
        compute_id: &str,
        boundary: &GenerationBoundary,
        input_tensors: &[Option<Tensor>],
        output_tensors: &[Option<Tensor>],
        mapping: ComputeMapping,
    ) {
        let pe = self.pes[mapping.pe_idx].name.clone();
        debug!("  Mapping compute node {compute_id} to {pe}");
        let input_views = boundary
            .inputs
            .iter()
            .enumerate()
            .map(|(index, maybe_view)| {
                maybe_view.as_ref().map(|view| {
                    let tensor = input_tensors[index]
                        .as_ref()
                        .expect("boundary input has a tensor");
                    self.register_edge(
                        tensor_id(tensor),
                        &format!("{compute_id}.{index}"),
                        EdgeKind::Data,
                    );
                    TensorViewSection {
                        offsets: view.offsets().to_vec(),
                        shape: view.shape().dims().to_vec(),
                    }
                })
            })
            .collect::<Vec<_>>();
        let output_views = boundary
            .outputs
            .iter()
            .enumerate()
            .map(|(index, maybe_view)| {
                maybe_view.as_ref().map(|view| {
                    let tensor = output_tensors[index]
                        .as_ref()
                        .expect("boundary output has a tensor");
                    let from = if boundary.outputs.len() == 1 {
                        compute_id.to_string()
                    } else {
                        format!("{compute_id}.{index}")
                    };
                    self.register_edge(&from, tensor_id(tensor), EdgeKind::Data);
                    TensorViewSection {
                        offsets: view.offsets().to_vec(),
                        shape: view.shape().dims().to_vec(),
                    }
                })
            })
            .collect::<Vec<_>>();
        self.nodes.push(NodeSection::Compute {
            id: compute_id.to_string(),
            program: mapping.program.into(),
            tiling: Some(mapping.tiling),
            pe: Some(pe),
            input_views: Some(input_views),
            output_views: Some(output_views),
        });
    }

    fn create_inputs_for_template(
        &mut self,
        template: &ProgramTemplate,
        outputs: &[Option<Tensor>],
    ) -> Result<Vec<Option<Tensor>>> {
        let dtype = *outputs[0].as_ref().expect("generated output").dtype();
        let optional_ratio = self.optional_input_ratio(template, dtype);
        template
            .create_inputs(
                outputs,
                self.args.expand_ratio,
                optional_ratio,
                &mut self.rng,
            )
            .map_err(boxed_error)
    }

    fn optional_input_ratio(&self, template: &ProgramTemplate, dtype: DataType) -> f64 {
        if matches!(template, ProgramTemplate::Gemm)
            && self.eligible_pe_indices(template, dtype, true).is_empty()
        {
            0.0
        } else {
            self.args.expand_ratio
        }
    }

    fn create_outputs_for_template(
        &mut self,
        template: &ProgramTemplate,
        inputs: &[Option<Tensor>],
    ) -> Result<Vec<Option<Tensor>>> {
        template
            .create_outputs(inputs, self.args.expand_ratio, &mut self.rng)
            .map_err(boxed_error)
    }

    fn add_ids_and_register(
        &mut self,
        base_name: &str,
        tensors: &mut [Option<Tensor>],
    ) -> Result<()> {
        for (idx, tensor) in tensors.iter_mut().enumerate() {
            let Some(tensor) = tensor else {
                continue;
            };
            if tensor.id().is_none() {
                tensor.set_id(format!("tensor_{base_name}_{idx}"));
            }
        }

        for tensor in tensors.iter_mut().flatten() {
            self.register_tensor(tensor)?;
        }
        Ok(())
    }

    fn make_tensor_with_dtype(
        &mut self,
        base_name: &str,
        shape: &Shape,
        dtype: &DataType,
    ) -> Result<Tensor> {
        let id = format!("tensor_{base_name}");
        let mut tensor = Tensor::from_shape(shape.clone(), dtype, 0)
            .map_err(boxed_error)?
            .with_id(id);
        self.register_tensor(&mut tensor)?;
        Ok(tensor)
    }

    fn register_tensor(&mut self, tensor: &mut Tensor) -> Result<()> {
        let id = tensor_id(tensor).to_string();
        if self
            .tensors
            .iter()
            .any(|registered| registered.id() == Some(id.as_str()))
        {
            return Ok(());
        }

        self.tensors.push(tensor.clone());
        Ok(())
    }

    fn register_edge(&mut self, from: &str, to: &str, kind: EdgeKind) {
        self.edges.push(EdgeSection {
            from: from.to_string(),
            to: to.to_string(),
            kind,
        });
    }

    fn next_id(&mut self, layer: isize, prefix: &str) -> String {
        let layer_prefix = if layer < 0 { "b" } else { "f" };
        let layer = layer.abs();
        let id = format!(
            "layer_{layer_prefix}{layer}_{prefix}_{}",
            self.next_node_idx
        );
        self.next_node_idx += 1;
        id
    }

    fn choose_expansion(&mut self) -> Result<Option<(usize, ProgramTemplate)>> {
        let eligible_templates = program_templates()
            .into_iter()
            .filter(|template| {
                self.template_weight(template) > 0.0
                    && self
                        .expansion_points
                        .iter()
                        .any(|point| self.point_allows_template(point, template))
            })
            .collect::<Vec<_>>();
        if eligible_templates.is_empty() {
            return Ok(None);
        }

        let template = self.choose_weighted_template(&eligible_templates);
        let candidate_indices = self
            .expansion_points
            .iter()
            .enumerate()
            .filter_map(|(idx, point)| self.point_allows_template(point, &template).then_some(idx))
            .collect::<Vec<_>>();
        let idx = candidate_indices[self.rng.random_range(0..candidate_indices.len())];
        Ok(Some((idx, template)))
    }

    fn point_allows_template(&self, point: &ExpansionPoint, template: &ProgramTemplate) -> bool {
        if !self.expansion_points.can_expand(point) {
            return false;
        }
        if self
            .eligible_pe_indices(template, *point.tensor.dtype(), false)
            .is_empty()
        {
            return false;
        }

        match template {
            ProgramTemplate::Add => true,
            ProgramTemplate::Gemm => point.tensor.shape().num_dims() >= 2,
            ProgramTemplate::ReduceMax => {
                point.direction == ExpansionDirection::Forward
                    && point.tensor.shape().num_dims() >= 2
            }
        }
    }

    fn eligible_pe_indices(
        &self,
        template: &ProgramTemplate,
        dtype: DataType,
        with_bias: bool,
    ) -> Vec<usize> {
        self.pes
            .iter()
            .enumerate()
            .filter_map(|(index, pe)| {
                let model = pe.model.compute_model();
                let supports = |op, inputs| {
                    model
                        .validate_work(&ComputeWork::operation(op, inputs, 1))
                        .is_ok()
                };
                let supported =
                    match template {
                        ProgramTemplate::Add => supports("add", vec![dtype, dtype]),
                        ProgramTemplate::Gemm => {
                            model.matrix_accumulators([dtype, dtype]).into_iter().any(
                                |accumulator| {
                                    accumulator == dtype || supports("convert", vec![accumulator])
                                },
                            ) && (!with_bias || supports("add", vec![dtype, dtype]))
                        }
                        ProgramTemplate::ReduceMax => supports("compare", vec![dtype, dtype]),
                    };
                supported.then_some(index)
            })
            .collect()
    }

    fn choose_weighted_template(&mut self, templates: &[ProgramTemplate]) -> ProgramTemplate {
        let total = templates
            .iter()
            .map(|template| self.template_weight(template))
            .sum::<f64>();
        let mut choice = self.rng.random_range(0.0..total);
        for template in templates {
            choice -= self.template_weight(template);
            if choice <= 0.0 {
                return template.clone();
            }
        }
        templates.last().unwrap().clone()
    }

    fn template_weight(&self, template: &ProgramTemplate) -> f64 {
        match template {
            ProgramTemplate::Add => self.args.weight_add,
            ProgramTemplate::Gemm => self.args.weight_gemm,
            ProgramTemplate::ReduceMax => self.args.weight_reduce_max,
        }
    }

    fn random_shape(&mut self) -> Result<Shape> {
        let mut rank_min = self.args.init_rank_min;
        if self.args.weight_gemm > 0.0 {
            rank_min = rank_min.max(2);
        }
        if self.args.weight_reduce_max > 0.0 {
            rank_min = rank_min.max(2);
        }

        let rank = self.rng.random_range(rank_min..=self.args.init_rank_max);
        let dims: Vec<usize> = (0..rank)
            .map(|_| {
                self.rng
                    .random_range(self.args.init_dim_min..=self.args.init_dim_max)
            })
            .collect();
        Shape::new(&dims).map_err(boxed_error)
    }
}

fn program_templates() -> [ProgramTemplate; NUM_TEMPLATES] {
    [
        ProgramTemplate::Add,
        ProgramTemplate::Gemm,
        ProgramTemplate::ReduceMax,
    ]
}

const NUM_TEMPLATES: usize = 3;

fn template_index(template: &ProgramTemplate) -> usize {
    match template {
        ProgramTemplate::Add => 0,
        ProgramTemplate::Gemm => 1,
        ProgramTemplate::ReduceMax => 2,
    }
}

fn template_name(template: &ProgramTemplate) -> &'static str {
    match template {
        ProgramTemplate::Add => "add",
        ProgramTemplate::Gemm => "gemm",
        ProgramTemplate::ReduceMax => "reduce_max",
    }
}

fn build_program_for_boundary(
    op: &ProgramTemplate,
    boundary: &GenerationBoundary,
    matrix_accumulator: DataType,
) -> Result<ProgramSpec> {
    op.build_program(&boundary.inputs, &boundary.outputs, matrix_accumulator)
        .map_err(boxed_error)
}

fn validate_args(args: &Cli) -> Result<()> {
    validate_weight("--weight-add", args.weight_add)?;
    validate_weight("--weight-gemm", args.weight_gemm)?;
    validate_weight("--weight-reduce-max", args.weight_reduce_max)?;

    if args.weight_add == 0.0 && args.weight_gemm == 0.0 && args.weight_reduce_max == 0.0 {
        return Err(error_from_str(
            "at least one template weight must be non-zero",
        ));
    }

    if args.depth == 0 {
        return Err(error_from_str("--depth must be greater than zero"));
    }
    if args.max_compute_nodes == 0 {
        return Err(error_from_str(
            "--max-compute-nodes must be greater than zero",
        ));
    }
    if args.partition_size == Some(0) {
        return Err(error_from_str("--partition-size must be greater than zero"));
    }
    if !args.expand_ratio.is_finite() || args.expand_ratio <= 0.0 {
        return Err(error_from_str(
            "--expand-ratio must be finite and greater than zero",
        ));
    }
    if args.init_rank_min == 0 || args.init_rank_max < args.init_rank_min {
        return Err(error_from_str("invalid initial rank range"));
    }
    if args.init_dim_min == 0 || args.init_dim_max < args.init_dim_min {
        return Err(error_from_str("invalid initial dimension range"));
    }
    if args.tensor_align_bytes == 0 {
        return Err(error_from_str(
            "--tensor-align-bytes must be greater than zero",
        ));
    }
    if args.weight_gemm > 0.0 && args.init_rank_max < 2 {
        return Err(error_from_str(
            "--weight-gemm is positive but init_rank_max is less than 2",
        ));
    }
    if args.weight_reduce_max > 0.0 && args.init_rank_max < 2 {
        return Err(error_from_str(
            "--weight-reduce-max is positive but init_rank_max is less than 2",
        ));
    }

    Ok(())
}

#[derive(Debug, Clone, Parser)]
#[command(about = "Generate a random ML-like timetable YAML from a seed tensor")]
struct Cli {
    /// Platform file to generate timetable for
    #[arg(long)]
    platform: PathBuf,

    /// Timetable file to write
    #[arg(long)]
    out: PathBuf,

    /// Enable debug output
    #[arg(long)]
    debug: bool,

    /// Number of compute layers in the generated timetable
    #[arg(long, default_value_t = 4)]
    depth: usize,

    /// Maximum number of generated compute nodes
    #[arg(long, default_value_t = 10)]
    max_compute_nodes: usize,

    /// Maximum extent of one partition along its outer output axis. By
    /// default the generator makes roughly one partition per PE.
    #[arg(long)]
    partition_size: Option<usize>,

    /// Tensor scale as the graph grows away from the initial tensor.
    /// Values greater than 1 grow tensors, while values below 1 shrink them.
    #[arg(long, default_value_t = 0.6)]
    expand_ratio: f64,

    /// Weight of the Add compute node being created
    #[arg(long, default_value_t = 1.0)]
    weight_add: f64,

    /// Weight of the GEMM compute node being created
    #[arg(long, default_value_t = 1.0)]
    weight_gemm: f64,

    /// Weight of the max-reduction compute node being created
    #[arg(long, default_value_t = 1.0)]
    weight_reduce_max: f64,

    /// Seed for the random number generator
    #[arg(long, default_value_t = 0x5eed_1234)]
    seed: u64,

    /// Data type for initial tensor
    #[arg(long, default_value_t, value_enum)]
    init_dtype: DataType,

    /// Minimum number of dimensions in the initial tensor
    #[arg(long, default_value_t = 2)]
    init_rank_min: usize,

    /// Maximum number of dimensions in the initial tensor
    #[arg(long, default_value_t = 4)]
    init_rank_max: usize,

    /// Minimum value for any dimensions in the initial tensor
    #[arg(long, default_value_t = 2)]
    init_dim_min: usize,

    /// Maximum value for any dimensions in the initial tensor
    #[arg(long, default_value_t = 32)]
    init_dim_max: usize,

    /// Tensor alignment in memory layout
    #[arg(long, default_value_t = 256)]
    tensor_align_bytes: u64,
}

fn validate_weight(name: &str, weight: f64) -> Result<()> {
    if !weight.is_finite() || weight < 0.0 {
        return Err(error_from_str(format!(
            "{name} must be finite and non-negative, got {weight}"
        )));
    }
    Ok(())
}

/// The Frontier manages all the possible ExpansionPoints
/// that can be used to expand the Timetable.
#[derive(Debug)]
struct Frontier {
    points: Vec<ExpansionPoint>,
    tensors_with_producer: HashSet<String>,
    tensors_with_consumer: HashSet<String>,
    layers: LayerBounds,
}

impl Frontier {
    fn new(target_depth: usize) -> Self {
        Self {
            points: Vec::new(),
            tensors_with_producer: HashSet::new(),
            tensors_with_consumer: HashSet::new(),
            layers: LayerBounds::new(target_depth),
        }
    }

    fn swap_remove(&mut self, idx: usize) -> ExpansionPoint {
        self.points.swap_remove(idx)
    }

    fn len(&self) -> usize {
        self.points.len()
    }

    fn try_to_add_expansion_point(
        &mut self,
        tensor: Tensor,
        tensor_layer: isize,
        direction: ExpansionDirection,
    ) {
        let point = ExpansionPoint {
            tensor,
            tensor_layer,
            direction,
        };

        debug!("Try to create {point}");

        if !self.can_expand(&point) || self.contains(&point) {
            return;
        }

        self.points.push(point);

        debug!(
            "Expansion points now have {} possible expansion locations",
            self.len()
        );
    }

    fn record_expanded_point(&mut self, point: &ExpansionPoint) {
        self.layers.record(point.next_layer());
        match point.direction {
            ExpansionDirection::Backward => self.record_producer(&point.tensor),
            ExpansionDirection::Forward => self.record_consumer(&point.tensor),
        }
    }

    fn record_producer(&mut self, tensor: &Tensor) {
        debug!("Adding {} to tensors_with_producer", tensor_id(tensor));
        self.tensors_with_producer
            .insert(tensor_id(tensor).to_string());
    }

    fn record_consumer(&mut self, tensor: &Tensor) {
        debug!("Adding {} to tensors_with_consumer", tensor_id(tensor));
        self.tensors_with_consumer
            .insert(tensor_id(tensor).to_string());
    }

    fn can_expand(&self, point: &ExpansionPoint) -> bool {
        self.layers.can_create(point.next_layer()) && !self.is_expanded(point)
    }

    fn is_expanded(&self, point: &ExpansionPoint) -> bool {
        match point.direction {
            ExpansionDirection::Backward => self
                .tensors_with_producer
                .contains(tensor_id(&point.tensor)),
            ExpansionDirection::Forward => self
                .tensors_with_consumer
                .contains(tensor_id(&point.tensor)),
        }
    }

    fn contains(&self, point: &ExpansionPoint) -> bool {
        self.points.iter().any(|existing| {
            tensor_id(&existing.tensor) == tensor_id(&point.tensor)
                && existing.direction == point.direction
                && existing.tensor_layer == point.tensor_layer
        })
    }

    fn iter(&self) -> impl Iterator<Item = &ExpansionPoint> {
        self.points.iter()
    }
}

#[derive(Debug, Clone)]
struct ExpansionPoint {
    tensor: Tensor,
    tensor_layer: isize,
    direction: ExpansionDirection,
}

impl ExpansionPoint {
    fn next_layer(&self) -> isize {
        match self.direction {
            ExpansionDirection::Backward => self.tensor_layer - 1,
            ExpansionDirection::Forward => self.tensor_layer + 1,
        }
    }
}

impl Display for ExpansionPoint {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: layer {}, direction: {:?}",
            tensor_id(&self.tensor),
            self.tensor_layer,
            self.direction
        )
    }
}

fn tensor_id(tensor: &Tensor) -> &str {
    tensor.id().expect("generated tensors must have ids")
}

#[derive(Debug, Clone, Copy)]
struct LayerBounds {
    min: isize,
    max: isize,
    target_depth: usize,
}

impl LayerBounds {
    const fn new(target_depth: usize) -> Self {
        Self {
            min: 0,
            max: 0,
            target_depth,
        }
    }

    fn can_create(self, layer: isize) -> bool {
        if layer == 0 {
            return false;
        }

        let min = self.min.min(layer);
        let max = self.max.max(layer);
        (max - min) as usize <= self.target_depth
    }

    fn record(&mut self, layer: isize) {
        self.min = self.min.min(layer);
        self.max = self.max.max(layer);
    }
}

struct PlacementPlan {
    pe_idx: usize,
    program: ProgramSpec,
    tensors: Vec<Tensor>,
    tiling: TileSpec,
    prepared: Rc<PreparedTask>,
    affected_tasks: Vec<(usize, Rc<PreparedTask>)>,
}

struct ComputeMapping {
    pe_idx: usize,
    program: ProgramSpec,
    tiling: TileSpec,
}

fn build_full_tensor_boundary(
    input_tensors: &[Option<Tensor>],
    output_tensors: &[Option<Tensor>],
) -> GenerationBoundary {
    GenerationBoundary {
        inputs: input_tensors
            .iter()
            .cloned()
            .map(|tensor| tensor.map(TensorView::new_full))
            .collect(),
        outputs: output_tensors
            .iter()
            .cloned()
            .map(|tensor| tensor.map(TensorView::new_full))
            .collect(),
    }
}

fn slice_view(view: &TensorView, dim: usize, offset: usize, length: usize) -> Result<TensorView> {
    let mut shape = view.shape().dims().to_vec();
    let mut offsets = view.offsets().to_vec();
    shape[dim] = length;
    offsets[dim] += offset;
    TensorView::new(view.tensor().clone(), &shape, &offsets).map_err(boxed_error)
}

fn find_tiling_for_pe(
    compute_id: &str,
    program: &ProgramSpec,
    boundary: &GenerationBoundary,
    outer_axis: &str,
    pe: &ProcessingElement,
) -> Result<(TileSpec, Rc<PreparedTask>)> {
    let region = bind_boundary(program, None, boundary)?;
    let mut tiling = TileSpec {
        sizes: BTreeMap::from([(
            outer_axis.to_string(),
            region.extent(outer_axis).expect("generated outer axis"),
        )]),
    };
    loop {
        let bound = bind_boundary(program, Some(&tiling), boundary)?;
        let error = match pe.prepare_program(compute_id.to_string(), Rc::new(bound)) {
            Ok(prepared) => return Ok((tiling, prepared)),
            Err(error) => error,
        };
        let mut best: Option<(usize, TileSpec)> = None;
        for (axis, extent) in region.extents() {
            let current = tiling.sizes.get(axis).copied().unwrap_or(*extent);
            if current <= 1 {
                continue;
            }
            let mut candidate = tiling.clone();
            candidate.sizes.insert(axis.clone(), current.div_ceil(2));
            let bytes = bind_boundary(program, Some(&candidate), boundary)?.peak_sram_bytes()?;
            if best
                .as_ref()
                .is_none_or(|(best_bytes, _)| bytes < *best_bytes)
            {
                best = Some((bytes, candidate));
            }
        }
        let Some((_, smaller)) = best else {
            return Err(error_from_str(format!(
                "{compute_id} minimum tile requires supported resources: {error}"
            )));
        };
        tiling = smaller;
    }
}

fn bind_boundary(
    program: &ProgramSpec,
    tiling: Option<&TileSpec>,
    boundary: &GenerationBoundary,
) -> Result<PreparedProgram> {
    let collect = |views: &[Option<TensorView>]| {
        views
            .iter()
            .cloned()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| error_from_str("generated program has an unbound port"))
    };
    PreparedProgram::new(
        program,
        RegionBindings {
            inputs: collect(&boundary.inputs)?,
            outputs: collect(&boundary.outputs)?,
        },
        tiling,
    )
    .map_err(boxed_error)
}

fn prepare_boundary(
    pe: &ProcessingElement,
    id: &str,
    program: &ProgramSpec,
    tiling: Option<&TileSpec>,
    boundary: &GenerationBoundary,
) -> Result<Rc<PreparedTask>> {
    pe.prepare_program(
        id.to_string(),
        Rc::new(bind_boundary(program, tiling, boundary)?),
    )
    .map_err(boxed_error)
}

struct GeneratorPe {
    name: String,
    model: Rc<ProcessingElement>,
    memory_ranges: Vec<Range<u128>>,
}

struct SelectedCompute {
    node: usize,
    pe: usize,
    boundary: GenerationBoundary,
    prepared: Rc<PreparedTask>,
}

fn relocate_boundary(
    boundary: &GenerationBoundary,
    tensors: &[Tensor],
) -> Result<GenerationBoundary> {
    let relocate = |views: &[Option<TensorView>]| {
        views
            .iter()
            .map(|view| {
                view.as_ref()
                    .map(|view| {
                        let tensor = tensors
                            .iter()
                            .find(|tensor| tensor.id() == view.tensor().id())
                            .expect("registered tensor");
                        TensorView::new(tensor.clone(), view.shape().dims(), view.offsets())
                            .map_err(boxed_error)
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>>>()
    };
    Ok(GenerationBoundary {
        inputs: relocate(&boundary.inputs)?,
        outputs: relocate(&boundary.outputs)?,
    })
}

fn coalesce(mut ranges: Vec<Range<u128>>) -> Vec<Range<u128>> {
    ranges.sort_by_key(|range| range.start);
    let mut result: Vec<Range<u128>> = Vec::new();
    for range in ranges {
        if let Some(last) = result.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            result.push(range);
        }
    }
    result
}

impl Generator {
    fn allocate_tensors(
        &self,
        proposed_pe: usize,
        proposed: &GenerationBoundary,
    ) -> Result<Vec<Tensor>> {
        let mut users: BTreeMap<&str, Vec<(usize, &TensorView)>> = BTreeMap::new();
        for (pe, boundary) in self
            .selected
            .iter()
            .map(|selected| (selected.pe, &selected.boundary))
            .chain(std::iter::once((proposed_pe, proposed)))
        {
            for view in boundary.inputs.iter().chain(&boundary.outputs).flatten() {
                users
                    .entry(tensor_id(view.tensor()))
                    .or_default()
                    .push((pe, view));
            }
        }
        self.place_tensors(&users)
    }

    fn place_tensors(
        &self,
        users: &BTreeMap<&str, Vec<(usize, &TensorView)>>,
    ) -> Result<Vec<Tensor>> {
        if self.allocated_tensors > 0
            && let Ok(tensors) = self.place_tensors_preserving(users, self.allocated_tensors)
        {
            return Ok(tensors);
        }
        self.place_tensors_preserving(users, 0)
    }

    fn place_tensors_preserving(
        &self,
        users: &BTreeMap<&str, Vec<(usize, &TensorView)>>,
        keep: usize,
    ) -> Result<Vec<Tensor>> {
        let mut tensors = self.tensors.clone();
        // Keep the committed prefix in place; order only tensors needing
        // allocation.
        tensors[keep..].sort_by_key(|tensor| {
            (
                !users.contains_key(tensor_id(tensor)),
                std::cmp::Reverse(tensor.num_bytes()),
                tensor_id(tensor).to_string(),
            )
        });
        let mut free = self.physical.clone();
        let alignment = u128::from(self.args.tensor_align_bytes);
        for (index, tensor) in tensors.iter_mut().enumerate() {
            let uses = users.get(tensor_id(tensor)).map_or(&[][..], Vec::as_slice);
            let placed = if index < keep {
                self.reserve_tensor(tensor, uses, &mut free)
            } else {
                let fallback = uses.iter().flat_map(|(pe, view)| {
                    view.address_ranges().flat_map(move |range| {
                        let offset = range.start - u128::from(view.tensor().addr());
                        self.pes[*pe].memory_ranges.iter().map(move |region| {
                            region.start.saturating_sub(offset).div_ceil(alignment) * alignment
                        })
                    })
                });
                self.place_tensor(tensor, uses, &mut free, fallback)
            };
            if !placed {
                return Err(error_from_str(format!(
                    "tensor '{}' has no mapped placement within physical capacity ({} bytes, {} users)",
                    tensor_id(tensor),
                    tensor.num_bytes(),
                    uses.len()
                )));
            }
        }
        Ok(tensors)
    }

    fn place_tensor(
        &self,
        tensor: &mut Tensor,
        uses: &[(usize, &TensorView)],
        free: &mut Vec<Range<u128>>,
        fallback: impl Iterator<Item = u128>,
    ) -> bool {
        let alignment = u128::from(self.args.tensor_align_bytes);
        let starts = free
            .iter()
            .map(|range| range.start.div_ceil(alignment) * alignment)
            .collect::<Vec<_>>();
        let mut tried = BTreeSet::new();
        // Chaining leaves even the first fragmented fallback run untouched
        // until every cheap free-interval start has failed.
        for base in starts.into_iter().chain(fallback) {
            if !tried.insert(base) {
                continue;
            }
            let Ok(address) = u64::try_from(base) else {
                continue;
            };
            if tensor.set_addr(address).is_ok() && self.reserve_tensor(tensor, uses, free) {
                return true;
            }
        }
        false
    }

    fn reserve_tensor(
        &self,
        tensor: &Tensor,
        uses: &[(usize, &TensorView)],
        free: &mut Vec<Range<u128>>,
    ) -> bool {
        let base = u128::from(tensor.addr());
        let end = base + tensor.num_bytes() as u128;
        let Some(index) = free
            .iter()
            .position(|range| range.start <= base && end <= range.end)
        else {
            return false;
        };
        if !tensor.addr().is_multiple_of(self.args.tensor_align_bytes)
            || !uses.iter().all(|(pe, view)| {
                TensorView::new(tensor.clone(), view.shape().dims(), view.offsets()).is_ok_and(
                    |view| {
                        self.pes[*pe].model.validate_mapped_view(&view).is_ok()
                            && self.pes[*pe]
                                .memory_ranges
                                .iter()
                                .map(|range| view.num_access_bytes_in(range.clone()))
                                .sum::<usize>()
                                == view.layout().num_access_bytes()
                    },
                )
            })
        {
            return false;
        }
        let interval = free.remove(index);
        if interval.start < base {
            free.push(interval.start..base);
        }
        if end < interval.end {
            free.push(end..interval.end);
        }
        free.sort_by_key(|range| range.start);
        true
    }
}

fn error_from_str(message: impl Into<String>) -> Box<dyn Error> {
    boxed_error(io::Error::other(message.into()))
}

fn boxed_error(error: impl Error + 'static) -> Box<dyn Error> {
    Box::new(error)
}

struct GenerationBoundary {
    inputs: Vec<Option<TensorView>>,
    outputs: Vec<Option<TensorView>>,
}
impl GenerationBoundary {
    fn partition(
        &self,
        input_dims: &[Option<usize>],
        output_dim: usize,
        offset: usize,
        length: usize,
    ) -> Result<Self> {
        let mut inputs = self.inputs.clone();
        for (view, dim) in inputs.iter_mut().zip(input_dims) {
            if let (Some(view), Some(dim)) = (view, dim) {
                *view = slice_view(view, *dim, offset, length)?;
            }
        }
        let mut outputs = self.outputs.clone();
        for view in outputs.iter_mut().flatten() {
            *view = slice_view(view, output_dim, offset, length)?;
        }
        Ok(Self { inputs, outputs })
    }
}

type TemplateIo = (Vec<Option<Tensor>>, Vec<Option<Tensor>>);

#[cfg(test)]
mod tests {
    use std::iter::once;
    use std::path::PathBuf;

    use gwr_models::processing_element::tensors::Tensor;
    use gwr_models::processing_element::tensors::dtype::DataType;
    use gwr_platform::types::PlatformConfig;
    use yaml_serde::from_str;

    use super::templates::ExpansionDirection;
    use super::{Cli, ExpansionPoint, Frontier, Generator, generate, validate_args};

    fn valid_args() -> Cli {
        Cli {
            platform: PathBuf::from("platform.yaml"),
            out: PathBuf::from("timetable.yaml"),
            debug: false,
            depth: 1,
            max_compute_nodes: 1,
            partition_size: None,
            expand_ratio: 0.6,
            weight_add: 1.0,
            weight_gemm: 1.0,
            weight_reduce_max: 1.0,
            init_dtype: DataType::Fp32,
            seed: 0,
            init_rank_min: 3,
            init_rank_max: 4,
            init_dim_min: 2,
            init_dim_max: 4,
            tensor_align_bytes: 256,
        }
    }

    fn generator_platform() -> PlatformConfig {
        from_str(
            r"
memory_maps:
  - name: default
    devices: [hbm0]
operation_classes:
  add: { operations: [add], arity: 2 }
  compare: { operations: [compare], arity: 2 }
  matrix: { operations: [matrix_mac], arity: 2, flops_per_op: 2 }
processing_elements:
  - name: pe0
    memory_map: default
    config:
      compute:
        execution_units:
          vector:
            capabilities:
              - { class: add, ops_per_tick: 16 }
              - { class: compare, ops_per_tick: 16 }
          matrix:
            capabilities: [{ class: matrix, accumulator: fp32, ops_per_tick: 256 }]
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config:
      capacity_bytes: 16GiB
connections:
  - connect: [pe.pe0, mem.hbm0]
",
        )
        .unwrap()
    }

    fn placement_generator(first: &str, second: &str, bytes: usize) -> Generator {
        let yaml = format!(
            r"
operation_classes:
  add: {{ operations: [add], arity: 2 }}
memory_maps:
  - {{ name: first, devices: [{first}] }}
  - {{ name: second, devices: [{second}] }}
  - {{ name: unused, devices: [d] }}
processing_elements:
  - name: pe0
    memory_map: first
    config: &pe
      lsu_access_bytes: 1
      compute:
        execution_units:
          vector:
            capabilities: [{{ class: add, ops_per_tick: 1 }}]
  - {{ name: pe1, memory_map: second, config: *pe }}
  - {{ name: unused, memory_map: unused, config: *pe }}
memories:
  - {{ name: a, kind: hbm, base_address: 0, config: {{ capacity_bytes: {bytes} }} }}
  - {{ name: b, kind: hbm, base_address: {bytes}, config: {{ capacity_bytes: {bytes} }} }}
  - {{ name: c, kind: hbm, base_address: {twice}, config: {{ capacity_bytes: {bytes} }} }}
  - {{ name: d, kind: hbm, base_address: {thrice}, config: {{ capacity_bytes: {bytes} }} }}
fabrics:
  - {{ name: fabric, kind: functional, columns: 7, rows: 1, config: {{}} }}
connections:
  - connect: [pe.pe0, 'fabric.fabric@(0,0)']
  - connect: [pe.pe1, 'fabric.fabric@(1,0)']
  - connect: [pe.unused, 'fabric.fabric@(2,0)']
  - connect: [mem.a, 'fabric.fabric@(3,0)']
  - connect: [mem.b, 'fabric.fabric@(4,0)']
  - connect: [mem.c, 'fabric.fabric@(5,0)']
  - connect: [mem.d, 'fabric.fabric@(6,0)']
",
            twice = bytes * 2,
            thrice = bytes * 3
        );
        let mut args = valid_args();
        args.tensor_align_bytes = 1;
        Generator::new(args, &from_str(&yaml).unwrap()).unwrap()
    }

    fn add_selection(generator: &mut Generator, id: &str, input: &Tensor, output: &Tensor) {
        use super::*;
        let inputs = [Some(input.clone()), Some(input.clone())];
        let outputs = [Some(output.clone())];
        generator
            .create_partitioned_compute(
                id,
                &ProgramTemplate::Add,
                &build_full_tensor_boundary(&inputs, &outputs),
                &inputs,
                &outputs,
            )
            .unwrap();
    }

    #[test]
    fn placement_attempt_reuses_unchanged_preparation_after_prior_relocation() {
        use super::*;
        let mut generator = placement_generator("a, c", "b, c", 16);
        let x = Tensor::new(&[1], &DataType::Fp32, 0).unwrap().with_id("x");
        let y = x.clone().with_id("y");
        let z = x.clone().with_id("z");
        let w = x.clone().with_id("w");
        generator.tensors = vec![x.clone(), y.clone()];
        add_selection(&mut generator, "first", &x, &y);
        generator.tensors.push(z.clone());
        add_selection(&mut generator, "relocating", &x, &z);
        assert_eq!(
            generator
                .tensors
                .iter()
                .find(|t| t.id() == Some("x"))
                .unwrap()
                .addr(),
            32
        );
        let first = generator.selected[0].prepared.clone();
        let second = generator.selected[1].prepared.clone();
        generator.tensors.push(w.clone());
        add_selection(&mut generator, "unchanged", &x, &w);
        assert!(
            Rc::ptr_eq(&first, &generator.selected[0].prepared),
            "unchanged actual bindings must retain their preparation"
        );
        assert!(Rc::ptr_eq(&second, &generator.selected[1].prepared));
    }

    #[test]
    fn placement_attempt_replaces_preparation_when_shared_binding_moves() {
        use super::*;
        let mut generator = placement_generator("a, c", "b, c", 16);
        let x = Tensor::new(&[1], &DataType::Fp32, 0).unwrap().with_id("x");
        let y = x.clone().with_id("y");
        let z = x.clone().with_id("z");
        generator.tensors = vec![x.clone(), y.clone()];
        add_selection(&mut generator, "first", &x, &y);
        let first = generator.selected[0].prepared.clone();
        assert_eq!(
            generator
                .tensors
                .iter()
                .find(|t| t.id() == Some("x"))
                .unwrap()
                .addr(),
            0
        );
        generator.tensors.push(z.clone());
        add_selection(&mut generator, "relocating", &x, &z);
        assert_eq!(
            generator
                .tensors
                .iter()
                .find(|t| t.id() == Some("x"))
                .unwrap()
                .addr(),
            32
        );
        assert!(!Rc::ptr_eq(&first, &generator.selected[0].prepared));
        assert_eq!(generator.selected[0].prepared.id(), "first");
        assert_eq!(
            generator.selected[0].prepared.planned_traffic_bytes(),
            (8, 4)
        );
    }

    #[test]
    fn placement_attempt_rejection_preserves_committed_state() {
        use super::*;
        let mut generator = placement_generator("a", "b", 8);
        let x = Tensor::new(&[1], &DataType::Fp32, 0).unwrap().with_id("x");
        let y = x.clone().with_id("y");
        let z = x.clone().with_id("z");
        generator.tensors = vec![x.clone(), y.clone()];
        add_selection(&mut generator, "first", &x, &y);
        generator.tensors.push(z.clone());
        let prepared = generator.selected[0].prepared.clone();
        let tensors = format!("{:?}", generator.tensors);
        let nodes = yaml_serde::to_string(&generator.nodes).unwrap();
        let edges = yaml_serde::to_string(&generator.edges).unwrap();
        let last_pe = generator.last_pe_idx;
        let inputs = [Some(x.clone()), Some(x)];
        let outputs = [Some(z)];
        let error = generator
            .create_partitioned_compute(
                "rejected",
                &ProgramTemplate::Add,
                &build_full_tensor_boundary(&inputs, &outputs),
                &inputs,
                &outputs,
            )
            .unwrap_err();
        assert!(error.to_string().contains("no feasible placement"));
        assert_eq!(format!("{:?}", generator.tensors), tensors);
        assert_eq!(yaml_serde::to_string(&generator.nodes).unwrap(), nodes);
        assert_eq!(yaml_serde::to_string(&generator.edges).unwrap(), edges);
        assert_eq!(generator.allocated_tensors, 2);
        assert_eq!(generator.last_pe_idx, last_pe);
        assert_eq!(generator.selected.len(), 1);
        assert!(Rc::ptr_eq(&prepared, &generator.selected[0].prepared));
    }

    #[test]
    fn placement_free_start_skips_million_fragmented_fallback_runs() {
        use std::cell::Cell;

        use super::*;
        let generator = placement_generator("a", "b", 16_777_216);
        let mut tensor = Tensor::new(&[1_000_000, 2], &DataType::Fp32, 0)
            .unwrap()
            .with_id("fragmented");
        let view = TensorView::new(tensor.clone(), &[1_000_000, 1], &[0, 0]).unwrap();
        let visited = Cell::new(0);
        let fallback = view
            .address_ranges()
            .inspect(|_| {
                visited.set(visited.get() + 1);
                assert!(
                    visited.get() <= 100,
                    "free-start placement traversed fragmented fallback runs"
                );
            })
            .map(|range| 0u128.saturating_sub(range.start));
        let mut free = generator.physical.clone();
        assert!(generator.place_tensor(&mut tensor, &[(0, &view)], &mut free, fallback));
        assert_eq!(visited.get(), 0);
        assert_eq!(tensor.addr(), 0);
        assert_eq!(tensor.num_bytes(), 8_000_000);
        assert_eq!(free[0].start, 8_000_000);
    }

    #[test]
    fn placement_reuses_existing_fragmented_tensor_before_new_allocations() {
        use super::*;
        let mut generator = placement_generator("a", "b", 32_000_000);
        let kept = Tensor::new(&[1_000_000, 2], &DataType::Fp32, 16_000_000)
            .unwrap()
            .with_id("kept");
        let new = Tensor::new(&[1], &DataType::Fp32, 0)
            .unwrap()
            .with_id("new");
        let kept_view = TensorView::new(kept.clone(), &[1_000_000, 1], &[0, 0]).unwrap();
        let new_view = TensorView::new_full(new.clone());
        generator.tensors = vec![kept, new];
        generator.allocated_tensors = 1;
        let users = BTreeMap::from([
            ("kept", vec![(0, &kept_view)]),
            ("new", vec![(0, &new_view)]),
        ]);
        let placed = generator.place_tensors(&users).unwrap();
        assert_eq!(
            placed
                .iter()
                .find(|tensor| tensor.id() == Some("kept"))
                .unwrap()
                .addr(),
            16_000_000
        );
        assert_eq!(
            placed
                .iter()
                .find(|tensor| tensor.id() == Some("new"))
                .unwrap()
                .addr(),
            0
        );
    }

    #[test]
    fn placement_replays_when_preserving_existing_storage_blocks_new_tensor() {
        use super::*;
        let mut generator = placement_generator("a, c", "a, b", 16);
        let kept = Tensor::new(&[4], &DataType::Fp32, 0)
            .unwrap()
            .with_id("kept");
        let new = Tensor::new(&[8], &DataType::Fp32, 0)
            .unwrap()
            .with_id("new");
        let kept_view = TensorView::new_full(kept.clone());
        let new_view = TensorView::new_full(new.clone());
        generator.tensors = vec![kept, new];
        generator.allocated_tensors = 1;
        let users = BTreeMap::from([
            ("kept", vec![(0, &kept_view)]),
            ("new", vec![(1, &new_view)]),
        ]);
        assert!(generator.place_tensors_preserving(&users, 1).is_err());
        let placed = generator.place_tensors(&users).unwrap();
        assert_eq!(
            placed
                .iter()
                .find(|tensor| tensor.id() == Some("kept"))
                .unwrap()
                .addr(),
            32
        );
        assert_eq!(
            placed
                .iter()
                .find(|tensor| tensor.id() == Some("new"))
                .unwrap()
                .addr(),
            0
        );
        assert_eq!(
            generator.tensors[0].addr(),
            0,
            "tentative replay must not mutate committed storage"
        );
        assert_eq!(generator.allocated_tensors, 1);
    }

    #[test]
    fn placement_shared_full_view_uses_only_actual_users() {
        use super::*;
        let mut generator = placement_generator("a, c", "b, c", 16);
        let tensor = Tensor::new(&[4], &DataType::Fp32, 0)
            .unwrap()
            .with_id("shared");
        generator.tensors.push(tensor.clone());
        let view = TensorView::new_full(tensor);
        let users = BTreeMap::from([("shared", vec![(0, &view), (1, &view)])]);
        let placed = generator.place_tensors(&users).unwrap();
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].id(), Some("shared"));
        assert_eq!(placed[0].addr(), 32);
        assert_eq!(placed[0].num_bytes(), 16);
        // A later consumer may move an existing tensor without changing its ID.
        let first = generator
            .place_tensors(&BTreeMap::from([("shared", vec![(0, &view)])]))
            .unwrap();
        assert_eq!(first[0].addr(), 0);
        generator.tensors = first;
        generator.allocated_tensors = 1;
        assert_eq!(generator.place_tensors(&users).unwrap()[0].addr(), 32);
    }

    #[test]
    fn placement_disjoint_views_span_adjacent_devices() {
        use super::*;
        let mut generator = placement_generator("a", "b", 16);
        let tensor = Tensor::new(&[8], &DataType::Fp32, 0)
            .unwrap()
            .with_id("shared");
        generator.tensors.push(tensor.clone());
        let prefix = TensorView::new(tensor.clone(), &[4], &[0]).unwrap();
        let suffix = TensorView::new(tensor.clone(), &[4], &[4]).unwrap();
        let users = BTreeMap::from([("shared", vec![(0, &prefix), (1, &suffix)])]);
        let placed = generator.place_tensors(&users).unwrap();
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].addr(), 0);
        assert_eq!(placed[0].num_bytes(), 32);
        let full = TensorView::new_full(tensor);
        let error = generator
            .place_tensors(&BTreeMap::from([("shared", vec![(0, &full), (1, &full)])]))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no mapped placement within physical capacity")
        );
    }

    #[test]
    fn placement_packed_views_require_shared_byte_addressability() {
        use super::*;
        let mut generator = placement_generator("a", "b", 1);
        let tensor = Tensor::new(&[4], &DataType::Fp4, 0)
            .unwrap()
            .with_id("packed");
        generator.tensors.push(tensor.clone());
        let prefix = TensorView::new(tensor.clone(), &[2], &[0]).unwrap();
        let suffix = TensorView::new(tensor.clone(), &[2], &[2]).unwrap();
        let placed = generator
            .place_tensors(&BTreeMap::from([(
                "packed",
                vec![(0, &prefix), (1, &suffix)],
            )]))
            .unwrap();
        assert_eq!(placed[0].addr(), 0);
        assert_eq!(placed[0].num_bytes(), 2);
        let crossing = TensorView::new(tensor, &[3], &[1]).unwrap();
        assert!(
            generator
                .place_tensors(&BTreeMap::from([(
                    "packed",
                    vec![(0, &prefix), (1, &crossing)]
                )]))
                .is_err()
        );
    }

    #[test]
    fn validate_args_rejects_zero_partition_size() {
        let mut args = valid_args();
        args.partition_size = Some(0);

        let err = validate_args(&args).unwrap_err();

        assert!(format!("{err}").contains("--partition-size"));
    }

    #[test]
    fn validate_args_rejects_zero_depth() {
        let mut args = valid_args();
        args.depth = 0;

        let err = validate_args(&args).unwrap_err();

        assert!(format!("{err}").contains("--depth must be greater than zero"));
    }

    #[test]
    fn validate_args_rejects_zero_max_compute_nodes() {
        let mut args = valid_args();
        args.max_compute_nodes = 0;

        let err = validate_args(&args).unwrap_err();

        assert!(format!("{err}").contains("--max-compute-nodes"));
    }

    #[test]
    fn validate_args_rejects_invalid_expand_ratio() {
        let mut args = valid_args();
        args.expand_ratio = 0.0;

        let err = validate_args(&args).unwrap_err();

        assert!(format!("{err}").contains("--expand-ratio"));
    }

    #[test]
    fn expansion_points_allow_existing_layers_after_target_depth_is_reached() {
        fn point(id: &str, tensor_layer: isize, direction: ExpansionDirection) -> ExpansionPoint {
            ExpansionPoint {
                tensor: Tensor::new(&[2, 2, 2], &DataType::Fp32, 0)
                    .unwrap()
                    .with_id(id),
                tensor_layer,
                direction,
            }
        }

        let mut points = Frontier::new(2);
        let backward = point("backward", 0, ExpansionDirection::Backward);
        let forward = point("forward", 0, ExpansionDirection::Forward);

        assert!(points.can_expand(&backward));
        points.record_expanded_point(&backward);
        assert!(points.can_expand(&forward));
        points.record_expanded_point(&forward);

        assert!(points.can_expand(&point(
            "same_backward_layer",
            0,
            ExpansionDirection::Backward,
        )));
        assert!(points.can_expand(&point("same_forward_layer", 0, ExpansionDirection::Forward,)));
        assert!(!points.can_expand(&point(
            "too_deep_backward",
            -1,
            ExpansionDirection::Backward,
        )));
        assert!(!points.can_expand(&point("too_deep_forward", 1, ExpansionDirection::Forward,)));
    }

    #[test]
    fn default_and_sampled_seeds_generate_valid_timetables() {
        let platform = generator_platform();
        for seed in once(0x5eed_1234).chain(0..20) {
            let mut args = valid_args();
            args.depth = 4;
            args.max_compute_nodes = 10;
            args.init_dim_max = 32;
            args.seed = seed;

            let timetable = generate(Generator::new(args, &platform).unwrap()).unwrap();
            timetable.validate().unwrap();
        }
    }
}
