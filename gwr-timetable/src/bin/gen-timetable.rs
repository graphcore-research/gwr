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

use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{read_to_string, write};
use std::path::PathBuf;
use std::{fmt, io};

use byte_unit::{Byte, UnitType};
use clap::Parser;
use gwr_engine::types::SimError;
use gwr_models::processing_element::compute::{ComputeModel, ComputeWork, MatrixWorkShape};
use gwr_models::processing_element::estimate_peak_tile_sram;
use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
use gwr_models::processing_element::program::validate::{RegionBindings, validate_program};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{HasShape, Shape, Tensor, TensorView};
use gwr_platform::types::PlatformConfig;
use gwr_timetable::timetable_file::{
    EdgeKind, EdgeSection, NodeSection, TensorConfigSection, TensorViewSection, TimetableFile,
};
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
    pe_names: Vec<String>,
    pe_models: Vec<ComputeModel>,
    pe_sram_bytes: Vec<usize>,
    allocator: AddrAllocator,
    tensors: Vec<Tensor>,
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

        let pe_names = platform
            .processing_elements
            .as_ref()
            .ok_or_else(|| error_from_str("platform contains no processing_elements"))?
            .iter()
            .map(|pe| pe.name.clone())
            .collect::<Vec<_>>();
        if pe_names.is_empty() {
            return Err(error_from_str("platform contains zero processing elements"));
        }
        let pe_resources = platform
            .processing_elements
            .as_ref()
            .expect("checked processing elements")
            .iter()
            .map(|pe| {
                let config = pe.effective_config(&platform.operation_classes)?;
                Ok::<_, SimError>((
                    ComputeModel::new(&config.compute, &platform.operation_classes)?,
                    config.sram_bytes,
                ))
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(boxed_error)?;
        let (pe_models, pe_sram_bytes) = pe_resources.into_iter().unzip();

        let base_addr = platform
            .memories
            .as_ref()
            .and_then(|mems| mems.first())
            .map(|m| m.base_address)
            .ok_or_else(|| error_from_str("platform contains no memories"))?;

        let seed = args.seed;
        let target_depth = args.depth;
        let remaining_compute_nodes = args.max_compute_nodes;
        let tensor_align_bytes = args.tensor_align_bytes;
        Ok(Self {
            args,
            rng: StdRng::seed_from_u64(seed),
            pe_names,
            pe_models,
            pe_sram_bytes,
            allocator: AddrAllocator::new(base_addr, tensor_align_bytes),
            tensors: Vec::new(),
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

        validate_generated_node(template, &input_tensors, &output_tensors)?;
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
        let requested_size = self
            .args
            .partition_size
            .unwrap_or_else(|| extent.div_ceil(eligible.len()));
        let count = extent
            .div_ceil(requested_size)
            .min(self.remaining_compute_nodes);
        let program = build_program_for_boundary(template, boundary)?;
        let base = extent / count;
        let extra = extent % count;
        let mut offset = 0;
        let mut tilings = BTreeMap::<(usize, usize, usize), TileSpec>::new();
        for part in 0..count {
            let length = base + usize::from(part < extra);
            let mut inputs = boundary.inputs.clone();
            for (view, dim) in inputs.iter_mut().zip(input_dims) {
                if let (Some(view), Some(dim)) = (view, dim) {
                    *view = slice_view(view, *dim, offset, length)?;
                }
            }
            let mut outputs = boundary.outputs.clone();
            for view in outputs.iter_mut().flatten() {
                *view = slice_view(view, output_dim, offset, length)?;
            }
            let part_boundary = GenerationBoundary { inputs, outputs };
            template
                .validate(&part_boundary.inputs, &part_boundary.outputs)
                .map_err(boxed_error)?;
            let part_id = if count == 1 {
                compute_id.to_string()
            } else {
                format!("{compute_id}_part{part}")
            };
            let pe_idx = self.next_pe_idx(&eligible);
            let sram_bytes = self.pe_sram_bytes[pe_idx];
            // Packed views may occupy different physical bytes when the
            // partition starts at a different element alignment.
            let packed = part_boundary
                .inputs
                .iter()
                .chain(&part_boundary.outputs)
                .flatten()
                .any(|view| view.dtype().num_bits() < 8);
            let cache_key = (pe_idx, length, if packed { offset } else { 0 });
            let tiling = if let Some(cached) = tilings.get(&cache_key) {
                cached.clone()
            } else {
                let planned = find_tiling_for_pe(
                    &part_id,
                    &program,
                    &part_boundary,
                    axis,
                    sram_bytes,
                    &self.pe_models[pe_idx],
                )?;
                tilings.insert(cache_key, planned.clone());
                planned
            };
            self.create_compute(
                &part_id,
                &part_boundary,
                input_tensors,
                output_tensors,
                ComputeMapping {
                    pe_idx,
                    program: program.clone(),
                    tiling,
                },
            )?;
            offset += length;
        }
        Ok(count)
    }

    fn next_pe_idx(&mut self, eligible: &[usize]) -> usize {
        let pe_idx = eligible
            .iter()
            .copied()
            .find(|index| *index >= self.last_pe_idx)
            .unwrap_or(eligible[0]);
        self.last_pe_idx = (pe_idx + 1) % self.pe_names.len();
        pe_idx
    }

    fn create_compute(
        &mut self,
        compute_id: &str,
        boundary: &GenerationBoundary,
        input_tensors: &[Option<Tensor>],
        output_tensors: &[Option<Tensor>],
        mapping: ComputeMapping,
    ) -> Result<()> {
        let pe = self.pe_names[mapping.pe_idx].clone();
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
        Ok(())
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

        let num_bytes = u64::try_from(tensor.num_bytes()).map_err(boxed_error)?;
        let addr = self.allocator.alloc(num_bytes)?;
        tensor.set_addr(addr).map_err(boxed_error)?;
        self.nodes.push(NodeSection::Tensor {
            id,
            config: TensorConfigSection {
                addr,
                dtype: *tensor.dtype(),
                shape: tensor.shape().dims().to_vec(),
            },
        });
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
        let operations: &[&str] = match template {
            ProgramTemplate::Add => &["add"],
            ProgramTemplate::Gemm if with_bias => &["matrix_mac", "add"],
            ProgramTemplate::Gemm => &["matrix_mac"],
            ProgramTemplate::ReduceMax => &["compare"],
        };
        self.pe_models
            .iter()
            .enumerate()
            .filter_map(|(index, model)| {
                operations
                    .iter()
                    .all(|op| {
                        // Probe a unit matrix with explicit geometry. Actual
                        // generated tile sizes are checked during tiling.
                        let work = if *op == "matrix_mac" {
                            ComputeWork::matrix_mac(
                                vec![dtype, dtype],
                                dtype,
                                MatrixWorkShape {
                                    batches: 1,
                                    m: 1,
                                    n: 1,
                                    k: 1,
                                },
                            )
                        } else {
                            Ok(ComputeWork::operation(*op, vec![dtype, dtype], 1))
                        };
                        work.and_then(|work| model.validate_work(&work)).is_ok()
                    })
                    .then_some(index)
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
) -> Result<ProgramSpec> {
    op.build_program(&boundary.inputs, &boundary.outputs)
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

#[derive(Debug)]
struct AddrAllocator {
    next_addr: u128,
    align_bytes: u128,
}

impl AddrAllocator {
    fn new(base_addr: u64, align_bytes: u64) -> Self {
        debug_assert!(align_bytes > 0);
        Self {
            next_addr: u128::from(base_addr),
            align_bytes: u128::from(align_bytes),
        }
    }

    fn alloc(&mut self, num_bytes: u64) -> Result<u64> {
        let rem = self.next_addr % self.align_bytes;
        if rem != 0 {
            self.next_addr += self.align_bytes - rem;
        }
        let out = self.next_addr;
        self.next_addr = self
            .next_addr
            .checked_add(u128::from(num_bytes))
            .ok_or_else(|| error_from_str("tensor address allocation overflows"))?;
        if self.next_addr > u128::from(u64::MAX) + 1 {
            return Err(error_from_str("tensor address allocation exceeds u64"));
        }
        u64::try_from(out).map_err(boxed_error)
    }
}

fn validate_generated_node(
    template: &ProgramTemplate,
    inputs: &[Option<Tensor>],
    outputs: &[Option<Tensor>],
) -> Result<()> {
    let inputs = inputs
        .iter()
        .cloned()
        .map(|tensor| tensor.map(TensorView::new_full))
        .collect::<Vec<_>>();
    let outputs = outputs
        .iter()
        .cloned()
        .map(|tensor| tensor.map(TensorView::new_full))
        .collect::<Vec<_>>();
    template.validate(&inputs, &outputs).map_err(boxed_error)
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
    sram_bytes: usize,
    compute_model: &ComputeModel,
) -> Result<TileSpec> {
    let inputs = boundary
        .inputs
        .iter()
        .cloned()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| error_from_str("generated program has an unbound input"))?;
    let outputs = boundary
        .outputs
        .iter()
        .cloned()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| error_from_str("generated program has an unbound output"))?;
    let region = validate_program(
        program,
        &RegionBindings {
            inputs: inputs.clone(),
            outputs: outputs.clone(),
        },
    )
    .map_err(boxed_error)?;
    let mut tiling = TileSpec {
        sizes: BTreeMap::from([(
            outer_axis.to_string(),
            region.extent(outer_axis).expect("generated outer axis"),
        )]),
    };
    loop {
        let required =
            estimate_peak_tile_sram(program, &inputs, &outputs, &tiling).map_err(boxed_error)?;
        let compute_error = compute_model.validate_program(&region, Some(&tiling)).err();
        if required <= sram_bytes && compute_error.is_none() {
            return Ok(tiling);
        }
        let mut best: Option<(usize, TileSpec)> = None;
        for (axis, extent) in region.extents() {
            // Accumulator demand depends on output contents. Reducing only a
            // contraction chunk cannot make an unsupported output tile fit.
            if compute_error.is_some()
                && !program
                    .assignments
                    .iter()
                    .any(|assignment| assignment.axes.contains(axis))
            {
                continue;
            }
            let current = tiling.sizes.get(axis).copied().unwrap_or(*extent);
            if current <= 1 {
                continue;
            }
            let mut candidate = tiling.clone();
            candidate.sizes.insert(axis.clone(), current.div_ceil(2));
            let bytes = estimate_peak_tile_sram(program, &inputs, &outputs, &candidate)
                .map_err(boxed_error)?;
            if best
                .as_ref()
                .is_none_or(|(best_bytes, _)| bytes < *best_bytes)
            {
                best = Some((bytes, candidate));
            }
        }
        let Some((_, smaller)) = best else {
            if let Some(error) = compute_error {
                return Err(error_from_str(format!(
                    "{compute_id} minimum tile is unsupported: {error}"
                )));
            }
            return Err(error_from_str(format!(
                "{compute_id} minimum tile requires {required} SRAM bytes, available {sram_bytes}"
            )));
        };
        tiling = smaller;
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
",
        )
        .unwrap()
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
