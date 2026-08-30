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

pub(crate) fn partition_across_dimensions(
    dims: &[usize],
    candidate_dims: &[usize],
    requested: usize,
) -> PartitionSpecs {
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

        split_dims.push(SplitDimension {
            dim,
            num_elements: dim_extent,
            num_partitions: splits,
        });
        achieved_partitions *= splits;
        if achieved_partitions >= requested {
            break;
        }
    }

    if split_dims.is_empty() {
        return PartitionSpecs {
            full_partition: Some(
                dims.iter()
                    .enumerate()
                    .map(|(dim, num_elements)| DimPartition {
                        dim,
                        offset: 0,
                        num_elements: *num_elements,
                    })
                    .collect(),
            ),
            split_dims,
            next_partition: 0,
            num_partitions: 0,
        };
    }

    PartitionSpecs {
        full_partition: None,
        split_dims,
        next_partition: 0,
        num_partitions: achieved_partitions,
    }
}

#[must_use]
pub(crate) fn max_partitions_across_dimensions(dims: &[usize], candidate_dims: &[usize]) -> usize {
    candidate_dims
        .iter()
        .map(|dim| dims[*dim])
        .product::<usize>()
        .max(1)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DimPartition {
    pub dim: usize,
    pub offset: usize,
    pub num_elements: usize,
}

#[derive(Clone, Debug)]
struct SplitDimension {
    dim: usize,
    num_elements: usize,
    num_partitions: usize,
}

impl SplitDimension {
    fn partition(&self, index: usize) -> DimPartition {
        let base_num_elements = self.num_elements / self.num_partitions;
        let remainder = self.num_elements % self.num_partitions;

        DimPartition {
            dim: self.dim,
            offset: index * base_num_elements + index.min(remainder),
            num_elements: base_num_elements + usize::from(index < remainder),
        }
    }
}

#[derive(Clone, Debug)]
#[must_use]
pub(crate) struct PartitionSpecs {
    full_partition: Option<Vec<DimPartition>>,
    split_dims: Vec<SplitDimension>,
    next_partition: usize,
    num_partitions: usize,
}

impl Iterator for PartitionSpecs {
    type Item = Vec<DimPartition>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(partition) = self.full_partition.take() {
            return Some(partition);
        }
        if self.next_partition == self.num_partitions {
            return None;
        }

        let partition_index = self.next_partition;
        self.next_partition += 1;
        let mut divisor = self.num_partitions;
        Some(
            self.split_dims
                .iter()
                .map(|split_dim| {
                    divisor /= split_dim.num_partitions;
                    let index = partition_index / divisor % split_dim.num_partitions;
                    split_dim.partition(index)
                })
                .collect(),
        )
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.len();
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for PartitionSpecs {
    fn len(&self) -> usize {
        if self.full_partition.is_some() {
            1
        } else {
            self.num_partitions - self.next_partition
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_large_partitionings_lazily() {
        let mut partitions = partition_across_dimensions(&[usize::MAX], &[0], usize::MAX);

        assert_eq!(
            partitions.next(),
            Some(vec![DimPartition {
                dim: 0,
                offset: 0,
                num_elements: 1,
            }])
        );
        assert_eq!(
            partitions.next(),
            Some(vec![DimPartition {
                dim: 0,
                offset: 1,
                num_elements: 1,
            }])
        );
        assert_eq!(partitions.len(), usize::MAX - 2);
    }
}
