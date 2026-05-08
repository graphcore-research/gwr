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
//! Compute node operators are selected using the user-defined weights.
//! Once an operator is chosen, the generator builds input and output tensor
//! definitions and partitions the compute across processing elements.
//! The new input/output tensors are added as possible expansion points.
//! The generator iterates until enough compute nodes have been created.

use std::collections::HashSet;
use std::fmt::Display;
use std::path::PathBuf;
use std::{fs, io};

use byte_unit::{Byte, UnitType};
use clap::Parser;
use gwr_models::processing_element::operators::add::OperatorAdd;
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::operators::gemm::{
    OperatorGemm, gemm_rhs_shape, maybe_add_input_c,
};
use gwr_models::processing_element::operators::maxpool::{
    OperatorMaxPool, create_maxpool_op, maybe_add_indices_output,
};
use gwr_models::processing_element::operators::{
    ExpansionDirection, HasShape, Operator, Shape, Tensor, TensorPartition, partition_tensors,
};
use gwr_models::processing_element::task::ComputeOp;
use gwr_platform::types::PlatformConfig;
use gwr_timetable::timetable_file::{
    EdgeKind, EdgeSection, NodeSection, TensorConfigSection, TensorViewSection, TimetableFile,
    dtype_num_bytes,
};
use log::{Level, LevelFilter, Metadata, Record, debug, info};
use rand::prelude::*;
use rand::rngs::StdRng;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct SimpleLogger;

static LOGGER: SimpleLogger = SimpleLogger;

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{}", record.args());
        }
    }

    fn flush(&self) {}
}

fn init_logging(debug_enabled: bool) {
    let level = if debug_enabled {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };

    let _ = log::set_logger(&LOGGER);
    log::set_max_level(level);
}

const NUM_OPS: usize = 3;

fn op_templates() -> [ComputeOp; NUM_OPS] {
    [
        ComputeOp::Add,
        ComputeOp::Gemm,
        ComputeOp::MaxPool(OperatorMaxPool::new(&[1])),
    ]
}

fn op_index(compute_op: &ComputeOp) -> usize {
    match compute_op {
        ComputeOp::Add => 0,
        ComputeOp::Gemm => 1,
        ComputeOp::MaxPool(_) => 2,
    }
}

fn op_name(compute_op: &ComputeOp) -> &'static str {
    match compute_op {
        ComputeOp::Add => "add",
        ComputeOp::Gemm => "gemm",
        ComputeOp::MaxPool(_) => "maxpool",
    }
}

fn tensor_id(tensor: &Tensor) -> &str {
    tensor.id().expect("generated tensors must have ids")
}

fn create_op(
    compute_op: &ComputeOp,
    tensor: &Tensor,
    direction: ExpansionDirection,
    expand_ratio: f64,
) -> Result<ComputeOp> {
    match compute_op {
        ComputeOp::Add => Ok(ComputeOp::Add),
        ComputeOp::Gemm => Ok(ComputeOp::Gemm),
        ComputeOp::MaxPool(_) => {
            let operator =
                create_maxpool_op(tensor, direction, expand_ratio).map_err(boxed_error)?;
            Ok(ComputeOp::MaxPool(operator))
        }
    }
}

fn boxed_error(error: impl std::error::Error + 'static) -> Box<dyn std::error::Error> {
    Box::new(error)
}

fn error_from_str(message: impl Into<String>) -> Box<dyn std::error::Error> {
    boxed_error(io::Error::other(message.into()))
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

    /// Maximum number of generated compute nodes (ignoring partitions)
    #[arg(long, default_value_t = 10)]
    max_compute_nodes: usize,

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

    /// Weight of the MaxPool compute node being created
    #[arg(long, default_value_t = 1.0)]
    weight_maxpool: f64,

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

    /// If set will allocate compute to all PEs in round robin fashion.
    /// Otherwise, compute partitions will be distributed starting at the
    /// first PE.
    #[arg(long)]
    round_robin_pes: bool,
}

fn validate_weight(name: &str, weight: f64) -> Result<()> {
    if !weight.is_finite() || weight < 0.0 {
        return Err(error_from_str(format!(
            "{name} must be finite and non-negative, got {weight}"
        )));
    }
    Ok(())
}

fn validate_args(args: &Cli) -> Result<()> {
    validate_weight("--weight-add", args.weight_add)?;
    validate_weight("--weight-gemm", args.weight_gemm)?;
    validate_weight("--weight-maxpool", args.weight_maxpool)?;

    if args.weight_add == 0.0 && args.weight_gemm == 0.0 && args.weight_maxpool == 0.0 {
        return Err(error_from_str(
            "at least one operator weight must be non-zero",
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
    if args.weight_maxpool > 0.0 && args.init_rank_max < 3 {
        return Err(error_from_str(
            "--weight-maxpool is positive but init_rank_max is less than 3",
        ));
    }

    Ok(())
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: layer {}, direction: {:?}",
            tensor_id(&self.tensor),
            self.tensor_layer,
            self.direction
        )
    }
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

    fn iter(&self) -> impl Iterator<Item = &ExpansionPoint> {
        self.points.iter()
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
            ExpansionDirection::Backward => {
                debug!(
                    "Adding {} to tensors_with_producer",
                    tensor_id(&point.tensor)
                );
                self.tensors_with_producer
                    .insert(tensor_id(&point.tensor).to_string());
            }
            ExpansionDirection::Forward => {
                debug!(
                    "Adding {} to tensors_with_consumer",
                    tensor_id(&point.tensor)
                );
                self.tensors_with_consumer
                    .insert(tensor_id(&point.tensor).to_string());
            }
        }
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
    next_addr: u64,
    align_bytes: u64,
}

impl AddrAllocator {
    fn new(base_addr: u64, align_bytes: u64) -> Self {
        debug_assert!(align_bytes > 0);
        Self {
            next_addr: base_addr,
            align_bytes,
        }
    }

    fn alloc(&mut self, num_bytes: u64) -> u64 {
        let rem = self.next_addr % self.align_bytes;
        if rem != 0 {
            self.next_addr += self.align_bytes - rem;
        }
        let out = self.next_addr;
        self.next_addr += num_bytes;
        out
    }
}

fn validate_generated_node(
    compute_op: &ComputeOp,
    inputs: &[Option<Tensor>],
    outputs: &[Option<Tensor>],
) -> Result<()> {
    match compute_op {
        ComputeOp::Add => OperatorAdd {}
            .validate_tensors(inputs, outputs)
            .map_err(boxed_error),
        ComputeOp::Gemm => OperatorGemm {}
            .validate_tensors(inputs, outputs)
            .map_err(boxed_error),
        ComputeOp::MaxPool(operator) => operator
            .validate_tensors(inputs, outputs)
            .map_err(boxed_error),
    }
}

fn create_partitions_for_op(
    compute_op: &ComputeOp,
    input_tensors: &[Option<Tensor>],
    output_tensors: &[Option<Tensor>],
    num_partitions: usize,
) -> Result<Vec<TensorPartition>> {
    match compute_op {
        ComputeOp::Add => partition_tensors(
            &OperatorAdd {},
            input_tensors,
            output_tensors,
            num_partitions,
        )
        .map_err(boxed_error),
        ComputeOp::Gemm => partition_tensors(
            &OperatorGemm {},
            input_tensors,
            output_tensors,
            num_partitions,
        )
        .map_err(boxed_error),
        ComputeOp::MaxPool(operator) => {
            partition_tensors(operator, input_tensors, output_tensors, num_partitions)
                .map_err(boxed_error)
        }
    }
}

type OperatorIo = (Vec<Option<Tensor>>, Vec<Option<Tensor>>);

struct Generator {
    args: Cli,
    rng: StdRng,
    pe_names: Vec<String>,
    allocator: AddrAllocator,
    tensors: Vec<Tensor>,
    nodes: Vec<NodeSection>,
    edges: Vec<EdgeSection>,
    next_node_idx: usize,
    op_counts: [usize; NUM_OPS],
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
            allocator: AddrAllocator::new(base_addr, tensor_align_bytes),
            tensors: Vec::new(),
            nodes: Vec::new(),
            edges: Vec::new(),
            next_node_idx: 0,
            op_counts: [0; NUM_OPS],
            expansion_points: Frontier::new(target_depth),
            remaining_compute_nodes,
            last_pe_idx: 0,
        })
    }

    fn print_summary(&self) {
        info!(
            "Created {} tensors / {} nodes / {} edges",
            self.tensors.len(),
            self.nodes.len(),
            self.edges.len()
        );
        let [add_op, gemm_op, maxpool_op] = op_templates();
        info!(
            "Compute ops: add={} gemm={} maxpool={}",
            self.op_counts[op_index(&add_op)],
            self.op_counts[op_index(&gemm_op)],
            self.op_counts[op_index(&maxpool_op)]
        );
        let total_bytes = self
            .tensors
            .iter()
            .map(|tensor| dtype_num_bytes(tensor.dtype(), tensor.shape().num_elements()) as u64)
            .sum();
        info!(
            "Tensors use {total_bytes} bytes ({:.2})",
            Byte::from_u64(total_bytes).get_appropriate_unit(UnitType::Binary)
        );
    }

    fn expand_point(&mut self, point: &ExpansionPoint, compute_op: &ComputeOp) -> Result<()> {
        let next_layer = point.next_layer();

        let compute_id = self.next_id(next_layer, op_name(compute_op));

        debug!("\nExpand {point}: {}", point.tensor.shape());
        debug!("  Compute: {compute_id} (layer {next_layer}):");

        let (input_tensors, output_tensors) = match point.direction {
            ExpansionDirection::Backward => {
                self.create_backward_io(&compute_id, compute_op, point)?
            }
            ExpansionDirection::Forward => {
                self.create_forward_io(&compute_id, compute_op, point)?
            }
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

        // Choose a random number of PEs over which to partition this compute
        let requested_partitions = self.rng.random_range(1..=self.pe_names.len());
        let partitions = create_partitions_for_op(
            compute_op,
            &input_tensors,
            &output_tensors,
            requested_partitions,
        )?;

        debug!(
            "  Attempt to split into {requested_partitions} partitions resulted in {} partitions.",
            partitions.len()
        );

        if partitions.is_empty() {
            return Err(error_from_str("operator produced zero partitions"));
        }

        validate_generated_node(compute_op, &input_tensors, &output_tensors)?;
        self.create_partitioned_compute(
            &compute_id,
            compute_op,
            &partitions,
            &input_tensors,
            &output_tensors,
        )?;

        self.remaining_compute_nodes -= 1;
        self.op_counts[op_index(compute_op)] += 1;
        self.expansion_points.record_expanded_point(point);
        self.enqueue_followups(point, &input_tensors, &output_tensors);

        Ok(())
    }

    fn create_backward_io(
        &mut self,
        compute_id: &str,
        compute_op: &ComputeOp,
        point: &ExpansionPoint,
    ) -> Result<OperatorIo> {
        let mut output_tensors = vec![Some(point.tensor.clone())];
        if matches!(compute_op, ComputeOp::MaxPool(_)) {
            maybe_add_indices_output(&mut output_tensors, self.args.expand_ratio, &mut self.rng)
                .map_err(boxed_error)?;
        }
        self.add_ids_and_register(&format!("{compute_id}_output"), &mut output_tensors);

        let mut input_tensors = self.create_inputs_for_op(compute_op, &output_tensors)?;
        self.add_ids_and_register(&format!("{compute_id}_input"), &mut input_tensors);

        Ok((input_tensors, output_tensors))
    }

    fn create_forward_io(
        &mut self,
        compute_id: &str,
        compute_op: &ComputeOp,
        point: &ExpansionPoint,
    ) -> Result<OperatorIo> {
        let mut input_tensors = self.create_forward_inputs(compute_op, &point.tensor)?;
        self.add_ids_and_register(&format!("{compute_id}_input"), &mut input_tensors);
        let mut output_tensors = self.create_outputs_for_op(compute_op, &input_tensors)?;
        self.add_ids_and_register(&format!("{compute_id}_output"), &mut output_tensors);

        Ok((input_tensors, output_tensors))
    }

    fn create_forward_inputs(
        &mut self,
        compute_op: &ComputeOp,
        input_tensor: &Tensor,
    ) -> Result<Vec<Option<Tensor>>> {
        match compute_op {
            ComputeOp::Add => {
                let other = Tensor::new(input_tensor.shape().get_dims(), input_tensor.dtype(), 0);
                let inputs = vec![Some(input_tensor.clone()), Some(other)];
                Ok(inputs)
            }
            ComputeOp::Gemm => {
                let other_shape = gemm_rhs_shape(input_tensor).map_err(boxed_error)?;
                let other = Tensor::new(other_shape.get_dims(), input_tensor.dtype(), 0);
                let mut inputs = vec![Some(input_tensor.clone()), Some(other)];
                maybe_add_input_c(&mut inputs, self.args.expand_ratio, &mut self.rng)
                    .map_err(boxed_error)?;
                Ok(inputs)
            }
            ComputeOp::MaxPool(_) => Ok(vec![Some(input_tensor.clone())]),
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
            if tensor_id(input_tensor) == tensor_id(&point.tensor) {
                continue;
            }
            self.expansion_points.try_to_add_expansion_point(
                input_tensor.clone(),
                input_tensor_layer,
                ExpansionDirection::Backward,
            );
            self.expansion_points.try_to_add_expansion_point(
                input_tensor.clone(),
                input_tensor_layer,
                ExpansionDirection::Forward,
            );
        }

        for output_tensor in output_tensors.iter().flatten() {
            if tensor_id(output_tensor) == tensor_id(&point.tensor) {
                continue;
            }
            self.expansion_points.try_to_add_expansion_point(
                output_tensor.clone(),
                output_tensor_layer,
                ExpansionDirection::Backward,
            );
            self.expansion_points.try_to_add_expansion_point(
                output_tensor.clone(),
                output_tensor_layer,
                ExpansionDirection::Forward,
            );
        }
    }

    fn next_pe_idx(&mut self, partition_idx: usize) -> usize {
        if self.args.round_robin_pes {
            let pe_idx = self.last_pe_idx;
            self.last_pe_idx = (self.last_pe_idx + 1) % self.pe_names.len();
            pe_idx
        } else {
            partition_idx % self.pe_names.len()
        }
    }

    fn create_partitioned_compute(
        &mut self,
        compute_id: &str,
        compute_op: &ComputeOp,
        partitions: &[TensorPartition],
        input_tensors: &[Option<Tensor>],
        output_tensors: &[Option<Tensor>],
    ) -> Result<()> {
        let mut total_input_num_bytes = 0;
        for (partition_idx, partition) in partitions.iter().enumerate() {
            let pe_idx = self.next_pe_idx(partition_idx);
            let pe = self.pe_names[pe_idx].clone();
            let partition_compute_id = if partitions.len() == 1 {
                compute_id.to_string()
            } else {
                format!("{compute_id}_part_{partition_idx}")
            };

            debug!("  Mapping partition {partition_idx} to {pe}");

            let mut input_views = vec![None; partition.inputs.len()];

            let mut partition_input_num_bytes = 0;
            for (input_idx, input_view) in partition.inputs.iter().enumerate() {
                let Some(view) = input_view else {
                    continue;
                };
                input_views[input_idx] = Some(TensorViewSection {
                    offsets: view.offsets().get_dims().clone(),
                    shape: view.shape().get_dims().clone(),
                });

                if log::log_enabled!(Level::Debug) {
                    let shape = view.shape();
                    let dtype = view.tensor().dtype();
                    let num_elements = shape.num_elements();
                    let num_bytes = dtype_num_bytes(dtype, num_elements);
                    partition_input_num_bytes += num_bytes;
                    debug!("  Partitioned input {input_idx}: shape: {shape}, bytes: {num_bytes}");
                }

                let input_tensor = input_tensors
                    .get(input_idx)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| {
                        error_from_str(format!(
                            "partition for {partition_compute_id} has input {input_idx} but no input tensor"
                        ))
                    })?;
                self.register_edge(
                    tensor_id(input_tensor),
                    &format!("{partition_compute_id}.{input_idx}"),
                    EdgeKind::Data,
                );
            }

            if log::log_enabled!(Level::Debug) {
                total_input_num_bytes += partition_input_num_bytes;
                debug!("  Partition input bytes: {partition_input_num_bytes}");
            }

            let mut output_views = vec![None; partition.outputs.len()];
            for (output_idx, output_view) in partition.outputs.iter().enumerate() {
                let Some(view) = output_view else {
                    continue;
                };
                output_views[output_idx] = Some(TensorViewSection {
                    offsets: view.offsets().get_dims().clone(),
                    shape: view.shape().get_dims().clone(),
                });

                let output_tensor = output_tensors
                    .get(output_idx)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| {
                        error_from_str(format!(
                            "partition for {partition_compute_id} has output {output_idx} but no output tensor"
                        ))
                    })?;
                let from = if partition.outputs.len() == 1 {
                    partition_compute_id.clone()
                } else {
                    format!("{partition_compute_id}.{output_idx}")
                };
                self.register_edge(&from, tensor_id(output_tensor), EdgeKind::Data);
            }

            self.nodes.push(NodeSection::Compute {
                id: partition_compute_id,
                op: compute_op.clone(),
                pe: Some(pe),
                input_views,
                output_views,
            });
        }

        debug!("  Total inputs bytes: {total_input_num_bytes}");

        Ok(())
    }

    fn create_inputs_for_op(
        &mut self,
        compute_op: &ComputeOp,
        outputs: &[Option<Tensor>],
    ) -> Result<Vec<Option<Tensor>>> {
        match compute_op {
            ComputeOp::Add => OperatorAdd {}
                .create_inputs(outputs, self.args.expand_ratio, &mut self.rng)
                .map_err(boxed_error),
            ComputeOp::Gemm => OperatorGemm {}
                .create_inputs(outputs, self.args.expand_ratio, &mut self.rng)
                .map_err(boxed_error),
            ComputeOp::MaxPool(operator) => operator
                .create_inputs(outputs, self.args.expand_ratio, &mut self.rng)
                .map_err(boxed_error),
        }
    }

    fn create_outputs_for_op(
        &mut self,
        compute_op: &ComputeOp,
        inputs: &[Option<Tensor>],
    ) -> Result<Vec<Option<Tensor>>> {
        match compute_op {
            ComputeOp::Add => OperatorAdd {}
                .create_outputs(inputs, self.args.expand_ratio, &mut self.rng)
                .map_err(boxed_error),
            ComputeOp::Gemm => OperatorGemm {}
                .create_outputs(inputs, self.args.expand_ratio, &mut self.rng)
                .map_err(boxed_error),
            ComputeOp::MaxPool(operator) => operator
                .create_outputs(inputs, self.args.expand_ratio, &mut self.rng)
                .map_err(boxed_error),
        }
    }

    fn add_ids_and_register(&mut self, base_name: &str, tensors: &mut [Option<Tensor>]) {
        for (idx, tensor) in tensors.iter_mut().enumerate() {
            let Some(tensor) = tensor else {
                continue;
            };
            if tensor.id().is_none() {
                tensor.set_id(format!("tensor_{base_name}_{idx}"));
            }
        }

        for tensor in tensors.iter_mut().flatten() {
            self.register_tensor(tensor);
        }
    }

    fn register_tensor(&mut self, tensor: &mut Tensor) {
        let id = tensor_id(tensor).to_string();
        if self
            .tensors
            .iter()
            .any(|registered| registered.id() == Some(id.as_str()))
        {
            return;
        }

        let num_bytes = dtype_num_bytes(tensor.dtype(), tensor.shape().num_elements());
        let addr = self.allocator.alloc(num_bytes as u64);
        self.nodes.push(NodeSection::Tensor {
            id,
            config: TensorConfigSection {
                addr,
                dtype: *tensor.dtype(),
                shape: tensor.shape().get_dims().clone(),
            },
        });
        tensor.set_addr(addr);
        self.tensors.push(tensor.clone());
    }

    fn make_tensor_with_dtype(
        &mut self,
        base_name: &str,
        shape: &Shape,
        dtype: &DataType,
    ) -> Tensor {
        let id = format!("tensor_{base_name}");
        let mut tensor = Tensor::new(shape.get_dims(), dtype, 0).with_id(id);
        self.register_tensor(&mut tensor);
        tensor
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

    fn choose_expansion(&mut self) -> Result<Option<(usize, ComputeOp)>> {
        let eligible_ops = op_templates()
            .into_iter()
            .filter(|compute_op| {
                self.op_weight(compute_op) > 0.0
                    && self
                        .expansion_points
                        .iter()
                        .any(|point| self.point_allows_op(point, compute_op))
            })
            .collect::<Vec<_>>();
        if eligible_ops.is_empty() {
            return Ok(None);
        }

        let op_template = self.choose_weighted_op(&eligible_ops);
        let candidate_indices = self
            .expansion_points
            .iter()
            .enumerate()
            .filter_map(|(idx, point)| self.point_allows_op(point, &op_template).then_some(idx))
            .collect::<Vec<_>>();
        let idx = candidate_indices[self.rng.random_range(0..candidate_indices.len())];
        let point = self
            .expansion_points
            .iter()
            .nth(idx)
            .expect("candidate index should refer to an expansion point");
        let compute_op = create_op(
            &op_template,
            &point.tensor,
            point.direction,
            self.args.expand_ratio,
        )?;
        Ok(Some((idx, compute_op)))
    }

    fn point_allows_op(&self, point: &ExpansionPoint, compute_op: &ComputeOp) -> bool {
        if !self.expansion_points.can_expand(point) {
            return false;
        }

        match compute_op {
            ComputeOp::Add => true,
            ComputeOp::Gemm => point.tensor.shape().num_dims() >= 2,
            ComputeOp::MaxPool(_) => point.tensor.shape().num_dims() >= 3,
        }
    }

    fn choose_weighted_op(&mut self, compute_ops: &[ComputeOp]) -> ComputeOp {
        let total = compute_ops
            .iter()
            .map(|compute_op| self.op_weight(compute_op))
            .sum::<f64>();
        let mut choice = self.rng.random_range(0.0..total);
        for compute_op in compute_ops {
            choice -= self.op_weight(compute_op);
            if choice <= 0.0 {
                return compute_op.clone();
            }
        }
        compute_ops.last().unwrap().clone()
    }

    fn op_weight(&self, compute_op: &ComputeOp) -> f64 {
        match compute_op {
            ComputeOp::Add => self.args.weight_add,
            ComputeOp::Gemm => self.args.weight_gemm,
            ComputeOp::MaxPool(_) => self.args.weight_maxpool,
        }
    }

    fn random_shape(&mut self) -> Shape {
        let mut rank_min = self.args.init_rank_min;
        if self.args.weight_gemm > 0.0 {
            rank_min = rank_min.max(2);
        }
        if self.args.weight_maxpool > 0.0 {
            rank_min = rank_min.max(3);
        }

        let rank = self.rng.random_range(rank_min..=self.args.init_rank_max);
        let dims: Vec<usize> = (0..rank)
            .map(|_| {
                self.rng
                    .random_range(self.args.init_dim_min..=self.args.init_dim_max)
            })
            .collect();
        Shape::new(&dims)
    }
}

fn generate(mut generator: Generator) -> Result<TimetableFile> {
    let shape = generator.random_shape();
    let init_dtype = generator.args.init_dtype;
    let seed_tensor = generator.make_tensor_with_dtype("initial", &shape, &init_dtype);
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
        let Some((idx, compute_op)) = generator.choose_expansion()? else {
            break;
        };
        let point = generator.expansion_points.swap_remove(idx);
        generator.expand_point(&point, &compute_op)?;
    }

    generator.print_summary();

    Ok(TimetableFile {
        nodes: generator.nodes,
        edges: generator.edges,
    })
}

fn main() -> Result<()> {
    let args = Cli::parse();
    init_logging(args.debug);

    let platform_yaml = fs::read_to_string(&args.platform)
        .map_err(|e| error_from_str(format!("failed to read {}: {e}", args.platform.display())))?;
    let platform: PlatformConfig = serde_yaml::from_str(&platform_yaml).map_err(|e| {
        error_from_str(format!(
            "failed to parse platform YAML using gwr_platform::types::PlatformConfig: {e}"
        ))
    })?;

    let out_path = args.out.clone();
    let generator = Generator::new(args, &platform)?;
    let timetable = generate(generator)?;
    let yaml = serde_yaml::to_string(&timetable)
        .map_err(|e| error_from_str(format!("failed to serialise timetable YAML: {e}")))?;
    fs::write(&out_path, yaml)
        .map_err(|e| error_from_str(format!("failed to write {}: {e}", out_path.display())))?;
    info!("Wrote graph to {}", out_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_args() -> Cli {
        Cli {
            platform: PathBuf::from("platform.yaml"),
            out: PathBuf::from("timetable.yaml"),
            debug: false,
            depth: 1,
            max_compute_nodes: 1,
            expand_ratio: 0.6,
            weight_add: 1.0,
            weight_gemm: 1.0,
            weight_maxpool: 1.0,
            init_dtype: DataType::Fp32,
            seed: 0,
            init_rank_min: 3,
            init_rank_max: 4,
            init_dim_min: 2,
            init_dim_max: 4,
            tensor_align_bytes: 256,
            round_robin_pes: false,
        }
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
                tensor: Tensor::new(&[2, 2, 2], &DataType::Fp32, 0).with_id(id),
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
}
