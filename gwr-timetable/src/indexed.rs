// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Scenario-selected row movement lowered to ordinary transfer tasks.
//! Indices are deterministic simulation metadata, not numerical runtime values.
//! Every transfer still waits for the named selection producer to finish.
use std::collections::{BTreeMap, BTreeSet};

use gwr_engine::sim_error;
use gwr_engine::types::SimError;
use gwr_models::processing_element::tensors::Tensor;

use crate::timetable_file::{
    EdgeKind, EdgeSection, NodeSection, ProgramDefinition, ProgramSection, TensorConfigSection,
    TensorViewSection, TimetableFile,
};

/// Gather selected source rows into successive destination rows. Duplicate
/// source indices are permitted and each occurrence incurs physical traffic.
pub fn lower_gather(selection: &RowSelection) -> Result<TimetableFile, SimError> {
    lower_rows(selection, false)
}

/// Scatter successive source rows into selected destination rows. Duplicate
/// destination indices are rejected: an additive combine requires explicit
/// reduction work rather than unordered overlapping writes.
pub fn lower_scatter(selection: &RowSelection) -> Result<TimetableFile, SimError> {
    lower_rows(selection, true)
}

#[expect(
    clippy::too_many_lines,
    reason = "Gather and scatter share the same ordered row-validation and lowering pass."
)]
fn lower_rows(selection: &RowSelection, scatter: bool) -> Result<TimetableFile, SimError> {
    let RowSelection {
        id,
        pe,
        source,
        destination,
        rows,
        selection_producer,
        buffer_bytes,
    } = selection;
    if id.is_empty() || pe.is_empty() || selection_producer.is_empty() || *buffer_bytes == 0 {
        return sim_error!(
            "Indexed movement requires ID, PE, selection producer, and nonzero buffer"
        );
    }
    if source.shape.is_empty()
        || source.shape.len() != destination.shape.len()
        || source.shape[1..] != destination.shape[1..]
        || source.dtype != destination.dtype
    {
        return sim_error!("Indexed movement requires equal row shape and dtype");
    }
    let (indexed, contiguous) = if scatter {
        (destination, source)
    } else {
        (source, destination)
    };
    if contiguous.shape[0] != rows.len().max(1) {
        return sim_error!(
            "Indexed movement contiguous tensor row count must match selection (one unused row for empty selections)"
        );
    }
    if rows.iter().any(|row| *row >= indexed.shape[0]) {
        return sim_error!("Indexed movement row selection is out of bounds");
    }
    if scatter && rows.iter().collect::<BTreeSet<_>>().len() != rows.len() {
        return sim_error!("Scatter has duplicate destinations; use an explicit reduction combine");
    }
    Tensor::new(&source.shape, &source.dtype, source.addr)?;
    Tensor::new(&destination.shape, &destination.dtype, destination.addr)?;
    let input = format!("{id}_source");
    let output = format!("{id}_destination");
    let complete = format!("{id}_complete");
    let mut file = TimetableFile {
        atomic_effects: None,
        programs: BTreeMap::new(),
        nodes: vec![
            NodeSection::Tensor {
                id: input.clone(),
                config: source.clone(),
            },
            NodeSection::Tensor {
                id: output.clone(),
                config: destination.clone(),
            },
            empty_task(&complete, pe),
        ],
        edges: vec![dependency(selection_producer, &complete)],
    };
    let mut start = 0;
    while start < rows.len() {
        let mut end = start + 1;
        while end < rows.len() && rows[end - 1].checked_add(1) == Some(rows[end]) {
            end += 1;
        }
        let (src, dst) = if scatter {
            (start, rows[start])
        } else {
            (rows[start], start)
        };
        let view = |offset| {
            let mut offsets = vec![0; source.shape.len()];
            offsets[0] = offset;
            let mut shape = source.shape.clone();
            shape[0] = end - start;
            TensorViewSection { offsets, shape }
        };
        let transfer = format!("{id}_rows_{start}_{end}");
        file.nodes.push(NodeSection::Transfer {
            id: transfer.clone(),
            pe: Some(pe.clone()),
            buffer_bytes: *buffer_bytes,
            input_views: Some(vec![Some(view(src))]),
            output_views: Some(vec![Some(view(dst))]),
        });
        file.edges.extend([
            EdgeSection {
                from: input.clone(),
                to: transfer.clone(),
                kind: EdgeKind::Data,
            },
            EdgeSection {
                from: transfer.clone(),
                to: output.clone(),
                kind: EdgeKind::Data,
            },
            dependency(selection_producer, &transfer),
            dependency(&transfer, &complete),
        ]);
        start = end;
    }
    Ok(file)
}

/// Parameters shared by scenario-backed gather and scatter movement.
#[derive(Clone, Debug)]
pub struct RowSelection {
    pub id: String,
    pub pe: String,
    pub source: TensorConfigSection,
    pub destination: TensorConfigSection,
    pub rows: Vec<usize>,
    /// An existing graph task. The returned fragment includes dependency edges
    /// referencing this task and must be merged before full graph validation.
    pub selection_producer: String,
    pub buffer_bytes: usize,
}
fn dependency(from: &str, to: &str) -> EdgeSection {
    EdgeSection {
        from: from.into(),
        to: to.into(),
        kind: EdgeKind::Dependency,
    }
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
