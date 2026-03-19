// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::PathBuf;
use std::{fs, io};

use byte_unit::{Byte, UnitType};
use clap::Parser;
use gwr_models::processing_element::operators::add::OperatorAdd;
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::operators::gemm::OperatorGemm;
use gwr_models::processing_element::operators::{
    HasShape, Operator, Shape, Tensor, TensorPartition, partition_tensors,
};
use gwr_models::processing_element::task::ComputeOp;
use gwr_platform::types::PlatformConfig;
use gwr_timetable::timetable_file::{
    EdgeKind, EdgeSection, NodeSection, TensorConfigSection, TensorViewSection, TimetableFile,
    dtype_num_bytes,
};
use rand::prelude::*;
use rand::rngs::StdRng;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn serialized_op_name(op: ComputeOp) -> Result<String> {
    let value = serde_yaml::to_value(op).map_err(boxed_error)?;
    match value {
        serde_yaml::Value::String(name) => Ok(name),
        other => Err(error_from_str(format!(
            "expected ComputeOp to serialize to a string, got {other:?}"
        ))),
    }
}

fn boxed_error(error: impl std::error::Error + 'static) -> Box<dyn std::error::Error> {
    Box::new(error)
}

fn error_from_str(message: impl Into<String>) -> Box<dyn std::error::Error> {
    boxed_error(io::Error::other(message.into()))
}

#[derive(Debug, Clone, Parser)]
#[command(
    about = "Generate a random ML-like timetable YAML by working backwards from an output tensor"
)]
struct Cli {
    #[arg(long)]
    platform: PathBuf,

    #[arg(long)]
    out: PathBuf,

    #[arg(long)]
    debug: bool,

    #[arg(long, default_value_t = 4)]
    depth: usize,

    #[arg(long, default_value_t = 10)]
    graph_size: usize,

    #[arg(long, default_value_t = 0.6)]
    expand_ratio: f64,

    #[arg(long, default_value_t = 0.5)]
    gemm_rate: f64,

    #[arg(long, default_value_t, value_enum)]
    dtype: DataType,

    #[arg(long, default_value_t = 0x5eed_1234)]
    seed: u64,

    #[arg(long, default_value_t = 2)]
    output_rank_min: usize,

    #[arg(long, default_value_t = 4)]
    output_rank_max: usize,

    #[arg(long, default_value_t = 2)]
    output_dim_min: usize,

    #[arg(long, default_value_t = 32)]
    output_dim_max: usize,

    #[arg(long, default_value_t = 256)]
    tensor_align_bytes: u64,
}

#[derive(Debug, Clone)]
struct TensorDef {
    id: String,
    dtype: DataType,
    shape: Shape,
}

#[derive(Debug, Clone)]
struct ExpansionPoint {
    tensor: TensorDef,
    max_depth: usize,
}

#[derive(Debug)]
struct AddrAllocator {
    next_addr: u64,
}

impl AddrAllocator {
    fn new(base: u64) -> Self {
        Self { next_addr: base }
    }

    fn alloc(&mut self, num_bytes: u64, align: u64) -> u64 {
        let rem = self.next_addr % align;
        if rem != 0 {
            self.next_addr += align - rem;
        }
        let out = self.next_addr;
        self.next_addr += num_bytes;
        out
    }
}

fn validate_generated_node(
    op: ComputeOp,
    inputs: &[Option<Tensor>],
    output: Option<Tensor>,
) -> Result<()> {
    let outputs = vec![output];
    match op {
        ComputeOp::Add => OperatorAdd {}
            .validate_tensors(inputs, &outputs)
            .map_err(boxed_error),
        ComputeOp::Gemm => OperatorGemm {}
            .validate_tensors(inputs, &outputs)
            .map_err(boxed_error),
    }
}

fn create_partitions_for_op(
    op: ComputeOp,
    input_tensors: &[Option<Tensor>],
    output_tensors: &[Option<Tensor>],
    num_partitions: usize,
) -> Result<Vec<TensorPartition>> {
    match op {
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
    }
}

struct Generator {
    args: Cli,
    rng: StdRng,
    pe_names: Vec<String>,
    allocator: AddrAllocator,
    tensors: Vec<TensorDef>,
    nodes: Vec<NodeSection>,
    edges: Vec<EdgeSection>,
    next_node_idx: usize,
}

impl Generator {
    fn new(args: Cli, platform: &PlatformConfig) -> Result<Self> {
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
            .unwrap();

        if args.output_rank_min == 0 || args.output_rank_max < args.output_rank_min {
            return Err(error_from_str("invalid output rank range"));
        }
        if args.output_dim_min == 0 || args.output_dim_max < args.output_dim_min {
            return Err(error_from_str("invalid output dimension range"));
        }

        let seed = args.seed;
        Ok(Self {
            args,
            rng: StdRng::seed_from_u64(seed),
            pe_names,
            allocator: AddrAllocator::new(base_addr),
            tensors: Vec::new(),
            nodes: Vec::new(),
            edges: Vec::new(),
            next_node_idx: 0,
        })
    }

    fn run(mut self) -> Result<TimetableFile> {
        let target_depth = self.args.depth.max(1);
        let target_graph_size = self.args.graph_size.max(target_depth);

        let shape = self.random_output_shape();
        let output_tensor = self.make_tensor_node("output", shape)?;
        let mut remaining_compute_nodes = target_graph_size;
        let mut candidates = Vec::new();

        self.expand_tensor(
            &output_tensor,
            target_depth,
            &mut remaining_compute_nodes,
            &mut candidates,
        )?;

        while remaining_compute_nodes > 0 {
            let expandable = candidates
                .iter()
                .enumerate()
                .filter(|(_, candidate)| candidate.max_depth > 0)
                .map(|(idx, _)| idx)
                .collect::<Vec<_>>();
            if expandable.is_empty() {
                break;
            }

            let idx = *expandable.choose(&mut self.rng).unwrap();
            let candidate = candidates.swap_remove(idx);
            let branch_depth =
                self.choose_branch_depth(candidate.max_depth, remaining_compute_nodes);
            self.expand_tensor(
                &candidate.tensor,
                branch_depth,
                &mut remaining_compute_nodes,
                &mut candidates,
            )?;
        }

        self.print_summary();

        Ok(TimetableFile {
            nodes: self.nodes,
            edges: self.edges,
        })
    }

    fn print_summary(&self) {
        println!(
            "Created {} tensors / {} nodes / {} edges",
            self.tensors.len(),
            self.nodes.len(),
            self.edges.len()
        );
        let total_bytes = self
            .tensors
            .iter()
            .map(|tensor_def| {
                dtype_num_bytes(&tensor_def.dtype, tensor_def.shape.num_elements()) as u64
            })
            .sum();
        println!(
            "Tensors use {total_bytes} bytes ({})",
            Byte::from_u64(total_bytes).get_appropriate_unit(UnitType::Binary)
        );
    }

    /// Create a compute node and corresponding inputs that will populate the
    /// specified output tensor.
    fn expand_tensor(
        &mut self,
        output_tensor_def: &TensorDef,
        depth: usize,
        remaining_compute_nodes: &mut usize,
        candidates: &mut Vec<ExpansionPoint>,
    ) -> Result<()> {
        if depth == 0 {
            return Ok(());
        }
        if *remaining_compute_nodes == 0 {
            return Err(error_from_str(
                "insufficient graph_size to realise requested depth",
            ));
        }
        *remaining_compute_nodes -= 1;

        let op = self.choose_op_for_output(output_tensor_def);
        let output_tensor = Tensor::new(
            output_tensor_def.shape.get_dims(),
            &output_tensor_def.dtype,
            0,
        );
        let outputs = vec![Some(output_tensor.clone())];
        let input_tensors = self.create_inputs_for_op(op, &outputs)?;

        let compute_id = self.next_id(&serialized_op_name(op)?);

        if self.args.debug {
            println!("\n{compute_id}:");
        }

        let input_defs = self.create_input_defs(&compute_id, &input_tensors)?;

        // Partition across all PEs
        let num_partitions = self.pe_names.len();
        let partitions = create_partitions_for_op(op, &input_tensors, &outputs, num_partitions)?;

        if self.args.debug {
            println!(
                "  Attempt to split into {num_partitions} partitions resulted in {} partitions.",
                partitions.len()
            );
        }

        if partitions.is_empty() {
            return Err(error_from_str("operator produced zero partitions"));
        }
        self.create_partitioned_compute_and_inputs(
            &compute_id,
            op,
            &partitions,
            &input_defs,
            output_tensor_def,
        )?;
        validate_generated_node(op, &input_tensors, Some(output_tensor))?;

        if depth > 1 {
            let spine_idx = self.rng.random_range(0..input_defs.len());
            for (idx, input_def) in input_defs.into_iter().enumerate() {
                if idx == spine_idx {
                    self.expand_tensor(&input_def, depth - 1, remaining_compute_nodes, candidates)?;
                } else {
                    candidates.push(ExpansionPoint {
                        tensor: input_def,
                        max_depth: depth - 1,
                    });
                }
            }
        }

        Ok(())
    }

    fn create_input_defs(
        &mut self,
        compute_id: &str,
        input_tensors: &[Option<Tensor>],
    ) -> Result<Vec<TensorDef>> {
        let mut input_defs = Vec::new();
        for (input_idx, input) in input_tensors.iter().enumerate() {
            if input.is_none() {
                continue;
            }
            let input = input.as_ref().unwrap();
            let input_def = self.make_tensor_node(
                format!("{compute_id}_input_{input_idx}").as_str(),
                input.shape().clone(),
            )?;
            input_defs.push(input_def);

            if self.args.debug {
                let shape = input.shape();
                let dtype = input.dtype();
                let num_bytes = dtype_num_bytes(dtype, shape.num_elements());
                println!("  Input {input_idx}: shape: {shape}, bytes: {num_bytes}");
            }
        }
        Ok(input_defs)
    }

    fn create_partitioned_compute_and_inputs(
        &mut self,
        compute_id: &str,
        op: ComputeOp,
        partitions: &[TensorPartition],
        input_defs: &[TensorDef],
        output_tensor_def: &TensorDef,
    ) -> Result<()> {
        let mut total_input_num_bytes = 0;
        for (partition_idx, partition) in partitions.iter().enumerate() {
            let pe = self.pe_names[partition_idx % self.pe_names.len()].clone();
            let partition_compute_id = if partitions.len() == 1 {
                compute_id.to_string()
            } else {
                format!("{compute_id}_part_{partition_idx}")
            };

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

                if self.args.debug {
                    let shape = input_view.as_ref().unwrap().shape();
                    let dtype = partition.inputs[input_idx]
                        .as_ref()
                        .unwrap()
                        .tensor()
                        .dtype();
                    let num_elements = shape.num_elements();
                    let num_bytes = dtype_num_bytes(dtype, num_elements);
                    partition_input_num_bytes += num_bytes;
                    println!("  Partitioned input {input_idx}: shape: {shape}, bytes: {num_bytes}");
                }

                let input_def = &input_defs[input_idx];
                self.emit_edge(
                    &input_def.id,
                    &format!("{partition_compute_id}.{input_idx}"),
                    EdgeKind::Data,
                );
            }

            if self.args.debug {
                total_input_num_bytes += partition_input_num_bytes;
                println!("  Partition input bytes: {partition_input_num_bytes}");
            }

            let output_view = partition.outputs[0].as_ref().ok_or_else(|| {
                error_from_str(format!(
                    "partition for {partition_compute_id} is missing output 0"
                ))
            })?;

            let compute_node = NodeSection::Compute {
                id: partition_compute_id.clone(),
                op,
                pe: Some(pe.clone()),
                input_views,
                output_views: vec![Some(TensorViewSection {
                    offsets: output_view.offsets().get_dims().clone(),
                    shape: output_view.shape().get_dims().clone(),
                })],
            };

            self.emit_edge(&partition_compute_id, &output_tensor_def.id, EdgeKind::Data);

            self.nodes.push(compute_node);
        }

        if self.args.debug {
            println!("  Total inputs bytes: {total_input_num_bytes}");
        }

        Ok(())
    }

    fn create_inputs_for_op(
        &mut self,
        op: ComputeOp,
        outputs: &[Option<Tensor>],
    ) -> Result<Vec<Option<Tensor>>> {
        match op {
            ComputeOp::Add => OperatorAdd {}
                .create_inputs(outputs, self.args.expand_ratio, &mut self.rng)
                .map_err(boxed_error),
            ComputeOp::Gemm => OperatorGemm {}
                .create_inputs(outputs, self.args.expand_ratio, &mut self.rng)
                .map_err(boxed_error),
        }
    }

    fn make_tensor_node(&mut self, base_name: &str, shape: Shape) -> Result<TensorDef> {
        let dtype = self.args.dtype.clone();
        let num_bytes = dtype_num_bytes(&dtype, shape.num_elements());
        let addr = self
            .allocator
            .alloc(num_bytes as u64, self.args.tensor_align_bytes);
        let id = format!("tensor_{base_name}");
        self.nodes.push(NodeSection::Tensor {
            id: id.clone(),
            config: TensorConfigSection {
                addr,
                dtype: dtype.clone(),
                shape: shape.get_dims().clone(),
            },
        });
        let tensor_def = TensorDef { id, dtype, shape };
        self.tensors.push(tensor_def.clone());
        Ok(tensor_def)
    }

    fn emit_edge(&mut self, from: &str, to: &str, kind: EdgeKind) {
        self.edges.push(EdgeSection {
            from: from.to_string(),
            to: to.to_string(),
            kind,
        });
    }

    fn next_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}_{}", self.next_node_idx);
        self.next_node_idx += 1;
        id
    }

    fn choose_op_for_output(&mut self, output: &TensorDef) -> ComputeOp {
        if output.shape.num_dims() >= 2
            && self.rng.random::<f64>() < self.args.gemm_rate.clamp(0.0, 1.0)
        {
            ComputeOp::Gemm
        } else {
            ComputeOp::Add
        }
    }

    fn choose_branch_depth(&mut self, max_depth: usize, budget: usize) -> usize {
        let depth_cap = max_depth.min(budget).max(1);
        if depth_cap == 1 {
            1
        } else {
            self.rng.random_range(1..=depth_cap)
        }
    }

    fn random_output_shape(&mut self) -> Shape {
        let rank = self
            .rng
            .random_range(self.args.output_rank_min..=self.args.output_rank_max);
        let dims: Vec<usize> = (0..rank)
            .map(|_| {
                self.rng
                    .random_range(self.args.output_dim_min..=self.args.output_dim_max)
            })
            .collect();
        Shape::new(&dims)
    }
}

fn main() -> Result<()> {
    let args = Cli::parse();
    let platform_yaml = fs::read_to_string(&args.platform)
        .map_err(|e| error_from_str(format!("failed to read {}: {e}", args.platform.display())))?;
    let platform: PlatformConfig = serde_yaml::from_str(&platform_yaml).map_err(|e| {
        error_from_str(format!(
            "failed to parse platform YAML using gwr_platform::types::PlatformConfig: {e}"
        ))
    })?;

    let out_path = args.out.clone();
    let generator = Generator::new(args, &platform)?;
    let timetable = generator.run()?;
    let yaml = serde_yaml::to_string(&timetable)
        .map_err(|e| error_from_str(format!("failed to serialise timetable YAML: {e}")))?;
    fs::write(&out_path, yaml)
        .map_err(|e| error_from_str(format!("failed to write {}: {e}", out_path.display())))?;
    println!("Wrote graph to {}", out_path.display());
    Ok(())
}
