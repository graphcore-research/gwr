// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Pure, bounded GEMM compilation. Only the finite full/tail shapes are
//! lowered.
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::rc::Rc;

use gwr_engine::types::SimError;

use super::syntax::{BoundsMode, ExprSpec, IndexSpec};
use super::tile::{RegionTile, TileSchedule, plan_tiles};
use super::validate::{RegionBindings, validate_program};
use crate::processing_element::block_execution::BlockExecutionConfig;
use crate::processing_element::compute::{
    CapabilityKey, ComputeModel, ComputeWork, EngineBatch, ExecutionPlan, MatrixWorkShape,
};
use crate::processing_element::task::ProgramTaskConfig;
use crate::processing_element::tensors::dtype::DataType;
use crate::processing_element::tensors::{HasShape, TensorView};

/// Complete simultaneously reserved working window; accumulator is counted
/// once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockStorageDemand {
    pub input_slot_bytes: usize,
    pub input_slots: usize,
    pub accumulator_bytes: u64,
    pub output_slot_bytes: usize,
    pub output_slots: usize,
    pub scratch_bytes: usize,
    /// Padding for eight-byte alignment of operand regions and resident slots.
    pub alignment_bytes: usize,
    pub sram_bytes: usize,
}
#[derive(Clone, Debug)]
struct Geometry {
    inputs: Vec<(TensorView, Vec<String>)>,
    output: TensorView,
    output_axes: Vec<String>,
    m: String,
    n: String,
    k: String,
    dtype: DataType,
    k_extent: usize,
    k_step: usize,
    batches: BTreeMap<(usize, usize, usize), EngineBatch>,
    finals: BTreeMap<(usize, usize), ExecutionPlan>,
}
/// Immutable lazy program plan, independent of simulator resources.
#[derive(Clone, Debug)]
pub struct BlockPipelinePlan {
    storage: BlockStorageDemand,
    schedule: TileSchedule,
    geometry: Rc<Geometry>,
    outputs: usize,
    chunks: usize,
}
impl BlockPipelinePlan {
    #[must_use]
    pub fn storage(&self) -> &BlockStorageDemand {
        &self.storage
    }
    #[must_use]
    pub fn output_tile_count(&self) -> usize {
        self.outputs
    }
    #[must_use]
    pub fn input_chunk_count(&self) -> usize {
        self.chunks
    }
    pub fn output_tiles(&self) -> impl Iterator<Item = OutputTilePlan> + use<> {
        let geometry = self.geometry.clone();
        self.schedule
            .clone()
            .into_tiles()
            .enumerate()
            .map(move |(id, tile)| {
                let g = &geometry;
                let m = tile.output_intervals[&g.m].len();
                let n = tile.output_intervals[&g.n].len();
                OutputTilePlan {
                    id,
                    output: slice(&g.output, &g.output_axes, &tile.output_intervals),
                    finalisation: g.finals[&(m, n)].clone(),
                    tile,
                    geometry: g.clone(),
                    shape: vec![m, n],
                }
            })
    }
}
/// One output tile retains one accumulator across every reduction chunk.
#[derive(Clone, Debug)]
pub struct OutputTilePlan {
    id: usize,
    output: TensorView,
    finalisation: ExecutionPlan,
    tile: RegionTile,
    geometry: Rc<Geometry>,
    shape: Vec<usize>,
}
impl OutputTilePlan {
    #[must_use]
    pub fn output_tile_id(&self) -> usize {
        self.id
    }
    #[must_use]
    pub fn output_view(&self) -> &TensorView {
        &self.output
    }
    #[must_use]
    pub fn accumulator_shape(&self) -> &[usize] {
        &self.shape
    }
    #[must_use]
    pub fn accumulator_dtype(&self) -> DataType {
        self.geometry.dtype
    }
    #[must_use]
    pub fn accumulator_bytes(&self) -> u64 {
        (self.shape[0] * self.shape[1] * (self.geometry.dtype.num_bits() / 8)) as u64
    }
    #[must_use]
    pub fn finalisation(&self) -> &ExecutionPlan {
        &self.finalisation
    }
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.geometry.k_extent.div_ceil(self.geometry.k_step)
    }
    pub fn input_chunks(&self) -> impl Iterator<Item = InputChunkPlan> + use<> {
        let this = self.clone();
        (0..self.chunk_count()).map(move |index| {
            let g = &this.geometry;
            let mut intervals = this.tile.output_intervals.clone();
            let start = index * g.k_step;
            intervals.insert(
                g.k.clone(),
                start..start.saturating_add(g.k_step).min(g.k_extent),
            );
            let shape = MatrixWorkShape {
                m: intervals[&g.m].len(),
                n: intervals[&g.n].len(),
                k: intervals[&g.k].len(),
            };
            InputChunkPlan {
                output: this.id,
                index,
                operands: g
                    .inputs
                    .iter()
                    .map(|(v, a)| slice(v, a, &intervals))
                    .collect(),
                batch: g.batches[&(shape.m, shape.n, shape.k)].clone(),
                shape,
            }
        })
    }
}
/// Operand views and compute demand for one reduction step.
#[derive(Clone, Debug)]
pub struct InputChunkPlan {
    output: usize,
    index: usize,
    operands: Vec<TensorView>,
    batch: EngineBatch,
    shape: MatrixWorkShape,
}
impl InputChunkPlan {
    #[must_use]
    pub fn output_tile_id(&self) -> usize {
        self.output
    }
    #[must_use]
    pub fn chunk_index(&self) -> usize {
        self.index
    }
    #[must_use]
    pub fn operands(&self) -> &[TensorView] {
        &self.operands
    }
    #[must_use]
    pub fn batch(&self) -> &EngineBatch {
        &self.batch
    }
    #[must_use]
    pub fn matrix_shape(&self) -> MatrixWorkShape {
        self.shape
    }
}
fn error(message: &str) -> SimError {
    SimError(format!("block pipeline: {message}"))
}
fn mul(a: usize, b: usize) -> Result<usize, SimError> {
    a.checked_mul(b)
        .ok_or_else(|| error("storage/work count overflows"))
}
fn add(a: usize, b: usize) -> Result<usize, SimError> {
    a.checked_add(b)
        .ok_or_else(|| error("storage/work count overflows"))
}
fn axes(expr: &ExprSpec) -> Result<(&str, Vec<String>), SimError> {
    let ExprSpec::Read {
        tensor,
        indices,
        bounds: BoundsMode::RequireInRange,
    } = expr
    else {
        return Err(error("operands require direct in-range reads"));
    };
    let names = indices
        .iter()
        .map(|i| match i {
            IndexSpec::Axis(a) => Ok(a.clone()),
            _ => Err(error("indexed operands are unsupported")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if names.iter().collect::<BTreeSet<_>>().len() != names.len() {
        return Err(error("repeated operand axis"));
    }
    Ok((tensor, names))
}
fn slice(
    view: &TensorView,
    axes: &[String],
    intervals: &BTreeMap<String, Range<usize>>,
) -> TensorView {
    let shape = axes.iter().map(|a| intervals[a].len()).collect::<Vec<_>>();
    let offsets = axes
        .iter()
        .enumerate()
        .map(|(i, a)| view.offsets()[i] + intervals[a].start)
        .collect::<Vec<_>>();
    TensorView::new(view.tensor().clone(), &shape, &offsets).expect("validated subview")
}
/// Validate geometry and capabilities without allocating storage or issuing
/// work.
#[expect(
    clippy::too_many_lines,
    reason = "Keep matrix pipeline shape validation and derived geometry in one planner"
)]
#[expect(
    clippy::many_single_char_names,
    reason = "Use conventional matrix dimensions and operand names"
)]
pub fn plan_block_pipeline(
    config: &ProgramTaskConfig,
    model: &ComputeModel,
    limits: &BlockExecutionConfig,
) -> Result<BlockPipelinePlan, SimError> {
    limits.validate()?;
    let request = config
        .pipeline
        .as_ref()
        .ok_or_else(|| error("missing pipeline request"))?;
    request.validate()?;
    let p = &config.program;
    if p.inputs.len() != 2
        || p.outputs.len() != 1
        || !p.metadata_inputs.is_empty()
        || !p.indexed_metadata_inputs.is_empty()
        || p.local_round_policy.is_some()
        || !(1..=2).contains(&p.assignments.len())
    {
        return Err(error(
            "requires one dense contraction and optional conversion",
        ));
    }
    let region = validate_program(
        p,
        &RegionBindings {
            inputs: config.inputs.clone(),
            outputs: config.outputs.clone(),
        },
    )?;
    let a = &p.assignments[region.assignment_order[0]];
    let ExprSpec::MatrixMac {
        args,
        over,
        execution,
        quantization: None,
    } = &a.expr
    else {
        return Err(error("requires an unscaled matrix contraction"));
    };
    if args.len() != 2 || over.len() != 1 || a.indices_output.is_some() {
        return Err(error("requires one contraction axis"));
    }
    if execution
        .as_ref()
        .is_some_and(|e| e.sparsity != crate::processing_element::compute::Sparsity::Dense)
    {
        return Err(error("only dense execution is supported"));
    }
    let k = over[0].clone();
    let mut inputs = Vec::new();
    for expr in args {
        let (name, names) = axes(expr)?;
        let i = p
            .inputs
            .iter()
            .position(|n| n == name)
            .ok_or_else(|| error("matrix operands must be boundary inputs"))?;
        let view = &config.inputs[i];
        if !view.dtype().num_bits().is_multiple_of(8) {
            return Err(error("packed inputs are unsupported"));
        }
        if names.len() != view.shape().dims().len()
            || names
                .iter()
                .zip(view.shape().dims())
                .any(|(n, s)| region.extent(n) != Some(*s))
        {
            return Err(error("operand shape differs from axis extents"));
        }
        inputs.push((view.clone(), names));
    }
    let free = |i: usize, j: usize| {
        inputs[i]
            .1
            .iter()
            .filter(|n| *n != &k && !inputs[j].1.contains(n))
            .cloned()
            .collect::<Vec<_>>()
    };
    let m = free(0, 1);
    let n = free(1, 0);
    if m.len() != 1 || n.len() != 1 || !inputs.iter().all(|(_, axes)| axes.contains(&k)) {
        return Err(error("requires distinct matrix free axes"));
    }
    let (m, n) = (m[0].clone(), n[0].clone());
    let expected = inputs
        .iter()
        .flat_map(|(_, a)| a)
        .filter(|a| *a != &k)
        .cloned()
        .collect::<BTreeSet<_>>();
    if a.axes.iter().cloned().collect::<BTreeSet<_>>() != expected || a.axes.len() != expected.len()
    {
        return Err(error("output axes differ from matrix roles"));
    }
    let dtype = region.value(&a.primary_result()).unwrap().dtype;
    if execution
        .as_ref()
        .map(|e| e.accumulator)
        .is_none_or(|d| d != dtype)
    {
        return Err(error(
            "explicit accumulator precision must match destination",
        ));
    }
    if dtype.num_bits() % 8 != 0 || !config.outputs[0].dtype().num_bits().is_multiple_of(8) {
        return Err(error("packed accumulator/output storage is unsupported"));
    }
    if !model.has_explicit_capability(&CapabilityKey {
        op: "matrix_mac".into(),
        inputs: inputs.iter().map(|(view, _)| *view.dtype()).collect(),
        execution: execution.clone(),
    }) {
        return Err(error(
            "matrix requires an explicit operand dtype capability",
        ));
    }
    let converted = p.assignments.len() == 2;
    let output_axes = if converted {
        let ep = &p.assignments[region.assignment_order[1]];
        let ExprSpec::Call { op, args, attrs } = &ep.expr else {
            return Err(error("requires explicit final conversion"));
        };
        if op != "convert"
            || args.len() != 1
            || !attrs.is_empty()
            || ep.axes != a.axes
            || ep.output.as_ref() != p.outputs.first()
            || p.temporaries.len() != 1
            || dtype != DataType::Fp32
            || *config.outputs[0].dtype() != DataType::Fp16
        {
            return Err(error("only FP32 to FP16 final conversion is supported"));
        }
        let (source, mapping) = axes(&args[0])?;
        if source != a.primary_result() || mapping != a.axes {
            return Err(error("conversion must read the accumulator directly"));
        }
        ep.axes.clone()
    } else {
        if !p.temporaries.is_empty() || a.output.as_ref() != p.outputs.first() {
            return Err(error("matrix result must be the output"));
        }
        a.axes.clone()
    };
    if converted
        && !model.has_explicit_capability(&CapabilityKey {
            op: "convert".into(),
            inputs: vec![dtype],
            execution: None,
        })
    {
        return Err(error(
            "conversion requires an explicit operand dtype capability",
        ));
    }
    for axis in expected.iter().filter(|axis| *axis != &m && *axis != &n) {
        if config
            .tiling
            .as_ref()
            .and_then(|s| s.sizes.get(axis))
            .copied()
            .unwrap_or(0)
            != 1
        {
            return Err(error("batch axis tile size must be one"));
        }
    }
    let schedule = plan_tiles(&region, config.tiling.as_ref())?;
    let step = |axis: &str| {
        config
            .tiling
            .as_ref()
            .and_then(|s| s.sizes.get(axis))
            .copied()
            .unwrap_or(region.extents[axis])
            .min(region.extents[axis])
    };
    let variants = |axis: &str| {
        let full = step(axis);
        let tail = region.extents[axis] % full;
        if tail == 0 {
            vec![full]
        } else {
            vec![full, tail]
        }
    };
    let mut batches = BTreeMap::new();
    let mut finals = BTreeMap::new();
    for mm in variants(&m) {
        for nn in variants(&n) {
            let finalisation = if converted {
                model.lower(&ComputeWork::operation(
                    "convert",
                    vec![dtype],
                    mul(mm, nn)?,
                ))?
            } else {
                ExecutionPlan { stages: vec![] }
            };
            finals.insert((mm, nn), finalisation);
            for kk in variants(&k) {
                let work = ComputeWork::matrix_mac(
                    inputs.iter().map(|(v, _)| *v.dtype()).collect(),
                    MatrixWorkShape {
                        m: mm,
                        n: nn,
                        k: kk,
                    },
                )?
                .with_execution(execution.clone());
                let plan = model.lower(&work)?;
                if plan.stages.len() != 1 || plan.stages[0].len() != 1 {
                    return Err(error("matrix must lower to one engine batch"));
                }
                batches.insert((mm, nn, kk), plan.stages[0][0].clone());
            }
        }
    }
    // Every K variant must remain executable by the retained engine. A tail
    // may have a different fastest engine, but cannot migrate its accumulator.
    for mm in variants(&m) {
        for nn in variants(&n) {
            let keys = variants(&k);
            let common = batches[&(mm, nn, keys[0])]
                .alternatives
                .iter()
                .map(|(engine, _)| *engine)
                .filter(|engine| {
                    keys.iter().all(|kk| {
                        batches[&(mm, nn, *kk)]
                            .alternatives
                            .iter()
                            .any(|(candidate, _)| candidate == engine)
                    })
                })
                .collect::<BTreeSet<_>>();
            if common.is_empty() {
                return Err(error("no common retained engine for all reduction chunks"));
            }
            for kk in keys {
                let batch = batches.get_mut(&(mm, nn, kk)).unwrap();
                batch
                    .alternatives
                    .retain(|(engine, _)| common.contains(engine));
                (batch.engine, batch.ticks) = batch.alternatives[0];
            }
        }
    }
    let align = |bytes: usize| add(bytes, 7).map(|padded| padded / 8 * 8);
    let operand_bytes = inputs
        .iter()
        .map(|(view, axes)| {
            axes.iter()
                .try_fold(view.dtype().num_bits() / 8, |size, axis| {
                    mul(size, step(axis))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let raw_input_bytes = operand_bytes
        .iter()
        .try_fold(0, |sum, bytes| add(sum, *bytes))?;
    let input_slot_bytes = operand_bytes
        .iter()
        .try_fold(0, |sum, bytes| add(sum, align(*bytes)?))?;
    let cells = mul(step(&m), step(&n))?;
    let accumulator = mul(cells, dtype.num_bits() / 8)?;
    let raw_output_bytes = mul(cells, config.outputs[0].dtype().num_bits() / 8)?;
    let output_slot_bytes = align(raw_output_bytes)?;
    let alignment_bytes = add(
        add(
            mul(input_slot_bytes - raw_input_bytes, request.input_buffers)?,
            mul(output_slot_bytes - raw_output_bytes, request.output_buffers)?,
        )?,
        align(accumulator)? - accumulator,
    )?;
    let sram_bytes = add(
        add(
            mul(input_slot_bytes, request.input_buffers)?,
            mul(output_slot_bytes, request.output_buffers)?,
        )?,
        align(accumulator)?,
    )?;
    let outputs = expected.iter().try_fold(1, |count, a| {
        mul(count, region.extents[a].div_ceil(step(a)))
    })?;
    let chunks = mul(outputs, region.extents[&k].div_ceil(step(&k)))?;
    let storage = BlockStorageDemand {
        input_slot_bytes,
        input_slots: request.input_buffers,
        accumulator_bytes: accumulator as u64,
        output_slot_bytes,
        output_slots: request.output_buffers,
        scratch_bytes: 0,
        alignment_bytes,
        sram_bytes,
    };
    Ok(BlockPipelinePlan {
        storage,
        schedule,
        geometry: Rc::new(Geometry {
            inputs,
            output: config.outputs[0].clone(),
            output_axes,
            k_extent: region.extents[&k],
            k_step: step(&k),
            m,
            n,
            k,
            dtype,
            batches,
            finals,
        }),
        outputs,
        chunks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing_element::compute::ComputeConfig;
    use crate::processing_element::tensors::Tensor;
    fn fixture(
        b: usize,
        m: usize,
        n: usize,
        k: usize,
    ) -> (ProgramTaskConfig, ComputeModel, BlockExecutionConfig) {
        let program = yaml_serde::from_str(
            r"
inputs: [a, b]
outputs: [c]
temporaries: {acc: {dtype: fp32}}
assignments:
- output: acc
  axes: [batch, m, n]
  expr:
    matrix_mac:
      execution: {instruction: tensor, accumulator: fp32}
      args:
      - read: {tensor: a, indices: [batch, m, k]}
      - read: {tensor: b, indices: [batch, k, n]}
      over: [k]
- output: c
  axes: [batch, m, n]
  expr: {convert: [{read: {tensor: acc, indices: [batch, m, n]}}]}
",
        )
        .unwrap();
        let view = |shape: &[usize], dtype, addr| {
            TensorView::new_full(Tensor::new(shape, &dtype, addr).unwrap())
        };
        let config = ProgramTaskConfig {
            id: "planner".into(),
            program,
            tiling: Some(
                yaml_serde::from_str("sizes: {batch: 1, m: 512, n: 512, k: 256}").unwrap(),
            ),
            pipeline: Some(
                yaml_serde::from_str("input_buffers: 2\noutput_buffers: 1\nprefetch_distance: 1")
                    .unwrap(),
            ),
            inputs: vec![
                view(&[b, m, k], DataType::Fp8, 0),
                view(&[b, k, n], DataType::Fp8, 1 << 40),
            ],
            outputs: vec![view(&[b, m, n], DataType::Fp16, 2 << 40)],
        };
        let hardware:ComputeConfig=yaml_serde::from_str(r"
engines:
  matrix:
    matrix_accumulator_capacity_bytes: 1048576
    capabilities:
    - {class: matrix, input_dtypes: [[fp8, fp8]], ops_per_tick: 8000, execution: {instruction: tensor, accumulator: fp32}}
  vector:
    capabilities:
    - {class: convert, input_dtypes: [[fp32]], ops_per_tick: 256}
").unwrap();
        let classes=yaml_serde::from_str("matrix: {operations: [matrix_mac], arity: 2, flops_per_op: 2}\nconvert: {operations: [convert], arity: 1, flops_per_op: 0}").unwrap();
        let limits=yaml_serde::from_str("max_compute_tiles: 1\nmax_load_chunks: 1\nmax_store_tiles: 1\ntransfer_port: lsu\naccumulator_storage: shared_sram").unwrap();
        (
            config,
            ComputeModel::new(&hardware, &classes).unwrap(),
            limits,
        )
    }
    #[test]
    fn fp32_window_and_work_are_exact() {
        let (config, model, limits) = fixture(512, 512, 512, 2048);
        let plan = plan_block_pipeline(&config, &model, &limits).unwrap();
        assert_eq!(
            plan.storage(),
            &BlockStorageDemand {
                input_slot_bytes: 262_144,
                input_slots: 2,
                accumulator_bytes: 1_048_576,
                output_slot_bytes: 524_288,
                output_slots: 1,
                scratch_bytes: 0,
                alignment_bytes: 0,
                sram_bytes: 2_097_152
            }
        );
        assert_eq!(plan.output_tile_count(), 512);
        assert_eq!(plan.input_chunk_count(), 4096);
        let mut macs = 0u64;
        let mut reads = 0usize;
        let mut writes = 0usize;
        let mut converts = 0usize;
        for tile in plan.output_tiles() {
            writes += tile.output_view().num_packed_bytes();
            converts += tile.output_view().num_elements();
            for chunk in tile.input_chunks() {
                macs += chunk.matrix_shape().logical_macs().unwrap() as u64;
                reads += chunk
                    .operands()
                    .iter()
                    .map(TensorView::num_packed_bytes)
                    .sum::<usize>();
            }
        }
        assert_eq!(
            (macs, converts, reads, writes),
            (274_877_906_944, 134_217_728, 1_073_741_824, 268_435_456)
        );
    }
    #[test]
    fn arbitrary_buffer_counts_charge_the_complete_window() {
        let (mut config, model, limits) = fixture(1, 512, 512, 256);
        for (i, o, total) in [
            (3, 1, 2_359_296),
            (4, 1, 2_621_440),
            (2, 2, 2_621_440),
            (3, 2, 2_883_584),
        ] {
            let r = config.pipeline.as_mut().unwrap();
            r.input_buffers = i;
            r.output_buffers = o;
            assert_eq!(
                plan_block_pipeline(&config, &model, &limits)
                    .unwrap()
                    .storage()
                    .sram_bytes,
                total
            );
        }
        config.pipeline.as_mut().unwrap().input_buffers = usize::MAX;
        assert!(plan_block_pipeline(&config, &model, &limits).is_err());
    }
    #[test]
    fn batched_tail_and_transpose_ranges_are_exact() {
        let (mut config, model, limits) = fixture(2, 5, 7, 11);
        config.tiling = Some(yaml_serde::from_str("sizes: {batch: 1, m: 3, n: 4, k: 4}").unwrap());
        // Physical B is transposed: preserve semantic k/n role independently.
        config.inputs[1] =
            TensorView::new_full(Tensor::new(&[2, 7, 11], &DataType::Fp8, 1 << 40).unwrap());
        if let ExprSpec::MatrixMac { args, .. } = &mut config.program.assignments[0].expr
            && let ExprSpec::Read { indices, .. } = &mut args[1]
        {
            indices.swap(1, 2);
        }
        let plan = plan_block_pipeline(&config, &model, &limits).unwrap();
        let mut output = BTreeSet::new();
        let mut macs = 0;
        for tile in plan.output_tiles() {
            for range in tile.output_view().address_ranges() {
                for address in range {
                    assert!(output.insert(address));
                }
            }
            let mut ks = 0;
            for chunk in tile.input_chunks() {
                let shape = chunk.matrix_shape();
                ks += shape.k;
                macs += shape.logical_macs().unwrap();
                assert_eq!(chunk.operands()[0].num_packed_bytes(), shape.m * shape.k);
                assert_eq!(chunk.operands()[1].num_packed_bytes(), shape.n * shape.k);
                let oi = &tile.tile.output_intervals;
                let kk = chunk.chunk_index() * 4..(chunk.chunk_index() * 4 + 4).min(11);
                let mut expected_a = BTreeSet::new();
                let mut expected_b = BTreeSet::new();
                for b in oi["batch"].clone() {
                    for k in kk.clone() {
                        for m in oi["m"].clone() {
                            expected_a.insert(((b * 5 + m) * 11 + k) as u128);
                        }
                        for n in oi["n"].clone() {
                            expected_b.insert((1u128 << 40) + ((b * 7 + n) * 11 + k) as u128);
                        }
                    }
                }
                assert_eq!(
                    chunk.operands()[0]
                        .address_ranges()
                        .flatten()
                        .collect::<BTreeSet<_>>(),
                    expected_a
                );
                assert_eq!(
                    chunk.operands()[1]
                        .address_ranges()
                        .flatten()
                        .collect::<BTreeSet<_>>(),
                    expected_b
                );
                for v in chunk.operands() {
                    assert_eq!(
                        v.address_ranges()
                            .map(|r| (r.end - r.start) as usize)
                            .sum::<usize>(),
                        v.num_packed_bytes()
                    );
                }
            }
            assert_eq!(ks, 11);
        }
        assert_eq!(output.len(), 2 * 5 * 7 * 2);
        assert_eq!(macs, 2 * 5 * 7 * 11);
    }
    #[test]
    fn packed_and_extra_assignments_reject() {
        let (mut config, model, limits) = fixture(2, 5, 7, 11);
        config.inputs[0] =
            TensorView::new_full(Tensor::new(&[2, 5, 11], &DataType::Fp4, 0).unwrap());
        assert!(
            plan_block_pipeline(&config, &model, &limits)
                .unwrap_err()
                .0
                .contains("packed")
        );
        let (mut config, model, limits) = fixture(2, 5, 7, 11);
        config
            .program
            .assignments
            .push(config.program.assignments[1].clone());
        assert!(plan_block_pipeline(&config, &model, &limits).is_err());
        let (mut config, model, limits) = fixture(2, 5, 7, 11);
        if let ExprSpec::MatrixMac { over, .. } = &mut config.program.assignments[0].expr {
            over.push("m".into());
        }
        assert!(plan_block_pipeline(&config, &model, &limits).is_err());
    }
    #[test]
    fn alias_and_missing_conversion_capability_reject() {
        let (mut config, model, limits) = fixture(2, 5, 7, 11);
        let no_conversion: ComputeConfig = yaml_serde::from_str("engines: {matrix: {capabilities: [{class: matrix, input_dtypes: [[fp8, fp8]], ops_per_tick: 1, execution: {instruction: tensor, accumulator: fp32}}]}}").unwrap();
        let classes =
            yaml_serde::from_str("matrix: {operations: [matrix_mac], arity: 2, flops_per_op: 2}")
                .unwrap();
        assert!(
            plan_block_pipeline(
                &config,
                &ComputeModel::new(&no_conversion, &classes).unwrap(),
                &limits
            )
            .is_err()
        );
        config.outputs[0] =
            TensorView::new_full(Tensor::new(&[2, 5, 7], &DataType::Fp16, 0).unwrap());
        assert!(
            plan_block_pipeline(&config, &model, &limits)
                .unwrap_err()
                .0
                .contains("overlaps")
        );
    }
    #[test]
    fn unsupported_selected_programs_reject_without_fallback() {
        let (mut config, model, limits) = fixture(2, 5, 7, 11);
        assert!(plan_block_pipeline(&config, &model, &limits).is_ok());
        config
            .tiling
            .as_mut()
            .unwrap()
            .sizes
            .insert("batch".into(), 2);
        assert!(
            plan_block_pipeline(&config, &model, &limits)
                .unwrap_err()
                .0
                .contains("batch axis")
        );
        config
            .tiling
            .as_mut()
            .unwrap()
            .sizes
            .insert("batch".into(), 1);
        if let ExprSpec::MatrixMac { args, .. } = &mut config.program.assignments[0].expr
            && let ExprSpec::Read { bounds, .. } = &mut args[0]
        {
            *bounds = BoundsMode::Exclude;
        }
        assert!(plan_block_pipeline(&config, &model, &limits).is_err());
    }
    #[test]
    fn packed_accumulator_and_output_reject_instead_of_reserving_zero_bytes() {
        let (mut config, _, limits) = fixture(1, 2, 2, 2);
        config.program.assignments.pop();
        config.program.temporaries.clear();
        config.program.assignments[0].output = Some("c".into());
        if let ExprSpec::MatrixMac { execution, .. } = &mut config.program.assignments[0].expr {
            execution.as_mut().unwrap().accumulator = DataType::Fp4;
        }
        config.outputs[0] =
            TensorView::new_full(Tensor::new(&[1, 2, 2], &DataType::Fp4, 2 << 40).unwrap());
        let hardware: ComputeConfig = yaml_serde::from_str("engines: {matrix: {matrix_accumulator_capacity_bytes: 1024, capabilities: [{class: matrix, input_dtypes: [[fp8,fp8]], ops_per_tick: 1, execution: {instruction: tensor, accumulator: fp4}}]}}").unwrap();
        let classes = yaml_serde::from_str("matrix: {operations: [matrix_mac], arity: 2}").unwrap();
        let model = ComputeModel::new(&hardware, &classes).unwrap();
        let error = plan_block_pipeline(&config, &model, &limits).unwrap_err();
        assert!(error.to_string().contains("packed"), "{error}");
    }

    #[test]
    fn pipeline_requires_explicit_matrix_and_conversion_dtype_capabilities() {
        let (config, _, limits) = fixture(1, 2, 2, 2);
        let classes = yaml_serde::from_str("matrix: {operations: [matrix_mac], arity: 2}\nconvert: {operations: [convert], arity: 1}").unwrap();
        for (matrix, convert) in [
            ("", "input_dtypes: [[fp32]],"),
            ("input_dtypes: [[fp8,fp8]],", ""),
        ] {
            let hardware: ComputeConfig = yaml_serde::from_str(&format!("engines: {{matrix: {{capabilities: [{{class: matrix, {matrix} ops_per_tick: 1, execution: {{instruction: tensor, accumulator: fp32}}}}]}}, vector: {{capabilities: [{{class: convert, {convert} ops_per_tick: 1}}]}}}}")).unwrap();
            let model = ComputeModel::new(&hardware, &classes).unwrap();
            let error = plan_block_pipeline(&config, &model, &limits).unwrap_err();
            assert!(error.to_string().contains("explicit"), "{error}");
        }
    }

    #[test]
    fn every_resident_operand_and_slot_is_eight_byte_aligned() {
        let (mut config, model, limits) = fixture(1, 3, 3, 3);
        config.tiling = Some(yaml_serde::from_str("sizes: {batch: 1, m: 3, n: 3, k: 3}").unwrap());
        let plan = plan_block_pipeline(&config, &model, &limits).unwrap();
        assert_eq!(plan.storage().input_slot_bytes, 32); // two9B operands, each rounded to16B
        assert_eq!(plan.storage().output_slot_bytes, 24); //18B rounded to24B
        assert_eq!(plan.storage().accumulator_bytes, 36);
        assert_eq!(plan.storage().sram_bytes, 128); //2*32 +24 + aligned36B state
    }
}
