// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Reproducible software collectives expanded into ordinary graph work.
//! The coordinator algorithm has no switch reduction offload. Scratch storage
//! is explicit and supplied by the caller; reduction uses platform compute.
use std::collections::BTreeMap;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;

use crate::timetable_file::{
    EdgeKind, EdgeSection, NodeSection, ProgramDefinition, ProgramSection, TensorConfigSection,
    TensorViewSection, TimetableFile,
};

/// Lower a collective with explicit tensor buffers, transfers, reduction work,
/// and a completion task waiting on every rank, including zero-count peers.
/// Scratch is used only by reductions and must contain `ranks * input_elements`
/// values followed by `input_elements` values at `scratch_addr`.
pub fn lower(
    id: &str,
    kind: &CollectiveKind,
    participants: &[Participant],
    scratch_addr: u64,
    buffer_bytes: usize,
) -> Result<TimetableFile, SimError> {
    if participants.is_empty() || buffer_bytes == 0 {
        return sim_error!("Collective requires participants and a nonzero transfer buffer");
    }
    let dtype = participants[0].input.dtype;
    if dtype.num_bits() < 8 {
        return sim_error!("Collectives currently require byte-aligned storage");
    }
    for rank in participants {
        if rank.input.shape.len() != 1
            || rank.output.shape.len() != 1
            || rank.input.dtype != dtype
            || rank.output.dtype != dtype
        {
            return sim_error!(
                "Collective participants require one-dimensional tensors with a common dtype"
            );
        }
    }
    let n = participants.len();
    let mut file = TimetableFile {
        atomic_effects: None,
        programs: BTreeMap::new(),
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    for (i, rank) in participants.iter().enumerate() {
        file.nodes.push(NodeSection::Tensor {
            id: format!("{id}_in_{i}"),
            config: nonempty_sentinel(&rank.input),
        });
        file.nodes.push(NodeSection::Tensor {
            id: format!("{id}_out_{i}"),
            config: nonempty_sentinel(&rank.output),
        });
    }
    let complete = format!("{id}_complete");
    file.nodes.push(empty_task(&complete, &participants[0].pe));
    for i in 0..n {
        dependency(&mut file, &format!("{id}_in_{i}"), &complete);
    }
    match kind {
        CollectiveKind::AllGather => {
            let size = participants[0].input.shape[0];
            let total = size
                .checked_mul(n)
                .ok_or_else(|| SimError("all-gather size overflows".into()))?;
            for rank in participants {
                if rank.input.shape[0] != size || rank.output.shape[0] != total {
                    return sim_error!(
                        "All-gather requires equal input counts and concatenated outputs"
                    );
                }
            }
            for source in 0..n {
                if size == 0 {
                    continue;
                }
                for (destination, rank) in participants.iter().enumerate() {
                    let name = format!("{id}_direct_{source}_{destination}");
                    transfer(
                        &mut file,
                        &name,
                        &rank.pe,
                        &format!("{id}_in_{source}"),
                        &format!("{id}_out_{destination}"),
                        0,
                        source * size,
                        size,
                        buffer_bytes,
                    );
                    dependency(&mut file, &name, &complete);
                }
            }
        }
        CollectiveKind::AllToAllV { counts } => {
            if counts.len() != n || counts.iter().any(|row| row.len() != n) {
                return sim_error!("All-to-allv count matrix must be ranks by ranks");
            }
            for i in 0..n {
                let input = counts[i]
                    .iter()
                    .try_fold(0usize, |sum, count| sum.checked_add(*count))
                    .ok_or_else(|| SimError("all-to-allv count overflows".into()))?;
                let output = counts
                    .iter()
                    .try_fold(0usize, |sum, row| sum.checked_add(row[i]))
                    .ok_or_else(|| SimError("all-to-allv count overflows".into()))?;
                if participants[i].input.shape[0] != input
                    || participants[i].output.shape[0] != output
                {
                    return sim_error!("All-to-allv rank {i} tensor lengths differ from counts");
                }
            }
            for source in 0..n {
                for (destination, rank) in participants.iter().enumerate() {
                    let count = counts[source][destination];
                    if count == 0 {
                        continue;
                    }
                    let src = counts[source][..destination].iter().sum();
                    let dst = counts[..source].iter().map(|row| row[destination]).sum();
                    let name = format!("{id}_direct_{source}_{destination}");
                    transfer(
                        &mut file,
                        &name,
                        &rank.pe,
                        &format!("{id}_in_{source}"),
                        &format!("{id}_out_{destination}"),
                        src,
                        dst,
                        count,
                        buffer_bytes,
                    );
                    dependency(&mut file, &name, &complete);
                }
            }
        }
        CollectiveKind::AllReduce | CollectiveKind::ReduceScatter => {
            let size = participants[0].input.shape[0];
            if size == 0 {
                return sim_error!("Reduction collectives require a nonempty input");
            }
            let output_size = match kind {
                CollectiveKind::ReduceScatter => {
                    if !size.is_multiple_of(n) {
                        return sim_error!(
                            "Reduce-scatter input count must divide evenly by ranks"
                        );
                    }
                    size / n
                }
                _ => size,
            };
            if participants
                .iter()
                .any(|rank| rank.input.shape[0] != size || rank.output.shape[0] != output_size)
            {
                return sim_error!("Reduction collective tensor lengths do not match");
            }
            let total = size
                .checked_mul(n)
                .ok_or_else(|| SimError("collective scratch size overflows".into()))?;
            let reduced_addr = scratch_addr
                .checked_add(
                    u64::try_from(
                        total
                            .checked_mul(dtype.num_bits() / 8)
                            .ok_or_else(|| SimError("scratch bytes overflow".into()))?,
                    )
                    .map_err(|_| SimError("scratch bytes exceed address space".into()))?,
                )
                .ok_or_else(|| SimError("scratch address overflow".into()))?;
            let scratch = format!("{id}_coordinator_scratch");
            let reduced = format!("{id}_reduced");
            file.nodes.push(NodeSection::Tensor {
                id: scratch.clone(),
                config: TensorConfigSection {
                    addr: scratch_addr,
                    dtype,
                    shape: vec![total],
                },
            });
            file.nodes.push(NodeSection::Tensor {
                id: reduced.clone(),
                config: TensorConfigSection {
                    addr: reduced_addr,
                    dtype,
                    shape: vec![size],
                },
            });
            for source in 0..n {
                transfer(
                    &mut file,
                    &format!("{id}_coordinator_gather_{source}"),
                    &participants[0].pe,
                    &format!("{id}_in_{source}"),
                    &scratch,
                    0,
                    source * size,
                    size,
                    buffer_bytes,
                );
            }
            let reduction = format!("{id}_coordinator_sum");
            // The flat scratch tensor is indexed with i + rank*size.
            let definition: ProgramDefinition = yaml_serde::from_str(&format!(r"
inputs: [A]
outputs: [Y]
extents: {{ i: {size}, rank: {n} }}
assignments:
  - output: Y
    axes: [i]
    expr:
      reduce:
        op: sum
        over: [rank]
        value: {{ read: {{ tensor: A, indices: [{{ affine: {{ terms: {{ i: 1, rank: {size} }}, offset: 0 }} }}] }} }}
")).map_err(|e| SimError(format!("collective reduction program: {e}")))?;
            file.nodes.push(NodeSection::Compute {
                pipeline: None,
                id: reduction.clone(),
                program: ProgramSection::Inline(Box::new(definition)),
                tiling: Some(gwr_models::processing_element::program::syntax::TileSpec {
                    sizes: BTreeMap::from([("i".into(), 1)]),
                }),
                pe: Some(participants[0].pe.clone()),
                input_views: None,
                output_views: None,
            });
            data(&mut file, &scratch, &reduction);
            data(&mut file, &reduction, &reduced);
            for (destination, rank) in participants.iter().enumerate() {
                let offset = if matches!(kind, CollectiveKind::ReduceScatter) {
                    destination * output_size
                } else {
                    0
                };
                let name = format!("{id}_coordinator_broadcast_{destination}");
                transfer(
                    &mut file,
                    &name,
                    &rank.pe,
                    &reduced,
                    &format!("{id}_out_{destination}"),
                    offset,
                    0,
                    output_size,
                    buffer_bytes,
                );
                dependency(&mut file, &name, &complete);
            }
        }
    }
    file.validate()?;
    Ok(file)
}

/// One rank's placement and contiguous one-dimensional tensor storage.
#[derive(Clone, Debug)]
pub struct Participant {
    pub pe: String,
    pub input: TensorConfigSection,
    pub output: TensorConfigSection,
}

/// The selected software algorithm is recorded in generated node IDs.
#[derive(Clone, Debug)]
pub enum CollectiveKind {
    AllReduce,
    AllGather,
    ReduceScatter,
    /// Per-source, per-destination element counts. Inputs and outputs are
    /// concatenated in destination and source rank order respectively.
    /// Empty ranks use shape `[0]` at this API; lowering emits an unused
    /// one-element dependency sentinel and no transfer for their payload.
    AllToAllV {
        counts: Vec<Vec<usize>>,
    },
}

fn empty_task(id: &str, pe: &str) -> NodeSection {
    NodeSection::Compute {
        pipeline: None,
        id: id.into(),
        pe: Some(pe.into()),
        program: ProgramSection::Inline(Box::new(ProgramDefinition {
            inputs: vec![],
            metadata_inputs: vec![],
            indexed_metadata_inputs: vec![],
            local_round_policy: None,
            outputs: vec![],
            temporaries: BTreeMap::new(),
            assignments: vec![],
            calls: vec![],
            extents: BTreeMap::new(),
        })),
        tiling: None,
        input_views: None,
        output_views: None,
    }
}
fn dependency(file: &mut TimetableFile, from: &str, to: &str) {
    file.edges.push(EdgeSection {
        from: from.into(),
        to: to.into(),
        kind: EdgeKind::Dependency,
    });
}
#[expect(clippy::too_many_arguments)]
fn transfer(
    file: &mut TimetableFile,
    id: &str,
    pe: &str,
    source: &str,
    destination: &str,
    src_offset: usize,
    dst_offset: usize,
    count: usize,
    buffer_bytes: usize,
) {
    file.nodes.push(NodeSection::Transfer {
        id: id.into(),
        pe: Some(pe.into()),
        buffer_bytes,
        input_views: Some(vec![Some(TensorViewSection {
            offsets: vec![src_offset],
            shape: vec![count],
        })]),
        output_views: Some(vec![Some(TensorViewSection {
            offsets: vec![dst_offset],
            shape: vec![count],
        })]),
    });
    data(file, source, id);
    data(file, id, destination);
}
fn data(file: &mut TimetableFile, from: &str, to: &str) {
    file.edges.push(EdgeSection {
        from: from.into(),
        to: to.into(),
        kind: EdgeKind::Data,
    });
}

// Empty participants carry one unused storage element as a dependency sentinel;
// this prevents zero-sized tensor allocation while issuing no payload requests.
fn nonempty_sentinel(config: &TensorConfigSection) -> TensorConfigSection {
    let mut config = config.clone();
    if config.shape == [0] {
        config.shape = vec![1];
    }
    config
}
