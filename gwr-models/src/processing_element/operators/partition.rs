// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::types::SimError;

use super::TensorView;

#[derive(Clone, Debug)]
pub struct TensorPartition {
    pub inputs: Vec<Option<TensorView>>,
    pub outputs: Vec<Option<TensorView>>,
}

impl TensorPartition {
    pub fn working_set_bytes(&self) -> Result<usize, SimError> {
        self.inputs
            .iter()
            .chain(&self.outputs)
            .flatten()
            .try_fold(0usize, |total, view| {
                total
                    .checked_add(view.layout().num_access_bytes())
                    .ok_or_else(|| SimError("Tensor partition byte count overflows".to_string()))
            })
    }
}

#[must_use]
pub(crate) fn partition_across_dimensions(
    dims: &[usize],
    candidate_dims: &[usize],
    requested: usize,
) -> Vec<Vec<DimPartition>> {
    let requested = requested.max(1);
    let mut split_dims = Vec::new();
    let mut achieved_partitions = 1usize;

    for &dim in candidate_dims {
        let dim_extent = dims[dim];
        if dim_extent <= 1 {
            continue;
        }

        let needed = requested.div_ceil(achieved_partitions).max(1);
        let splits = dim_extent.min(needed);
        if splits <= 1 {
            continue;
        }

        split_dims.push((dim, partition_into_ranges(dim_extent, splits)));
        achieved_partitions *= splits;
        if achieved_partitions >= requested {
            break;
        }
    }

    if split_dims.is_empty() {
        return vec![
            dims.iter()
                .enumerate()
                .map(|(dim, num_elements)| DimPartition {
                    dim,
                    offset: 0,
                    num_elements: *num_elements,
                })
                .collect(),
        ];
    }

    let mut partitions = vec![Vec::new()];
    for (dim, ranges) in split_dims {
        let mut next = Vec::with_capacity(partitions.len() * ranges.len());
        for base in &partitions {
            for (offset, num_elements) in &ranges {
                let mut partition = base.clone();
                partition.push(DimPartition {
                    dim,
                    offset: *offset,
                    num_elements: *num_elements,
                });
                next.push(partition);
            }
        }
        partitions = next;
    }
    partitions
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DimPartition {
    pub dim: usize,
    pub offset: usize,
    pub num_elements: usize,
}

fn partition_into_ranges(total: usize, requested: usize) -> Vec<(usize, usize)> {
    let partitions = requested.clamp(1, total.max(1));
    let base_range_size = total / partitions;
    let remainder = total % partitions;
    let mut start = 0;
    let mut ranges = Vec::with_capacity(partitions);

    for index in 0..partitions {
        let num_elements = base_range_size + usize::from(index < remainder);
        if num_elements != 0 {
            ranges.push((start, num_elements));
            start += num_elements;
        }
    }

    if ranges.is_empty() {
        ranges.push((0, total.max(1)));
    }
    ranges
}
