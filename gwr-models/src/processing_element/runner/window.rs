// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;

use super::{align8, eval_index};
use crate::processing_element::program::syntax::IndexSpec;
use crate::processing_element::program::tile::RegionTile;
use crate::processing_element::program::validate::ValidatedRegion;
use crate::processing_element::tensors::{HasShape, TensorView};

pub(super) fn window_input_view(
    view: &TensorView,
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ValidatedRegion,
) -> Result<TensorView, SimError> {
    window_input_view_for_chunk(view, indices, tile, region, &BTreeMap::default())?
        .ok_or_else(|| SimError("window input slice has no valid elements".into()))
}

pub(super) fn max_window_chunk_buffer_bytes(
    view: &TensorView,
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ValidatedRegion,
    over: &[String],
    available_sram: usize,
) -> Result<usize, SimError> {
    let mut peak = 0usize;
    let unpadded = window_all_in_range(indices, tile, region, view)?;
    // Full byte-aligned windows differ only by a coordinate translation.
    // Their physical byte count is constant; trailing partial chunks are
    // subsets of the first full chunk. Padding and packed byte alignment can
    // change that count, so retain exhaustive inspection for those cases.
    let chunks = if unpadded && view.dtype().num_bits().is_multiple_of(8) {
        1
    } else {
        usize::MAX
    };
    for chunk in tile.reduction_chunks_for(over).take(chunks) {
        if unpadded {
            let unique_lower_bound = indices.iter().zip(view.shape().dims()).try_fold(
                1usize,
                |product, (index, dimension)| {
                    if *dimension == 1 {
                        return Ok(product);
                    }
                    let axes = match index {
                        IndexSpec::Zero(_) => vec![],
                        IndexSpec::Axis(axis) => vec![axis.as_str()],
                        IndexSpec::Affine { affine } => {
                            affine.terms.keys().map(String::as_str).collect()
                        }
                    };
                    let varying = axes
                        .into_iter()
                        .map(|axis| {
                            chunk
                                .get(axis)
                                .or_else(|| tile.output_intervals.get(axis))
                                .map_or_else(|| region.extent(axis).unwrap(), Range::len)
                        })
                        .max()
                        .unwrap_or(1);
                    product
                        .checked_mul(varying)
                        .ok_or_else(|| SimError("window SRAM lower bound overflows".into()))
                },
            )?;
            let min_bytes = unique_lower_bound
                .checked_mul(view.dtype().num_bits())
                .ok_or_else(|| SimError("window SRAM lower bound overflows".into()))?
                .div_ceil(8);
            if min_bytes > available_sram {
                return sim_error!(
                    "window tile requires at least {min_bytes} SRAM bytes, available {available_sram}"
                );
            }
        }
        let (_, bytes) = window_chunk_access_ranges(view, indices, tile, region, &chunk)?;
        peak = peak.max(align8(bytes)?);
    }
    Ok(peak)
}

#[expect(clippy::too_many_lines)]
pub(super) fn window_chunk_access_ranges(
    view: &TensorView,
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ValidatedRegion,
    chunk: &BTreeMap<String, Range<usize>>,
) -> Result<(Vec<Range<u128>>, usize), SimError> {
    let dense = indices.iter().all(|index| {
        let IndexSpec::Affine { affine } = index else {
            return true;
        };
        let mut strides = affine
            .terms
            .iter()
            .map(|(axis, stride)| {
                let len = chunk
                    .get(axis)
                    .or_else(|| tile.output_intervals.get(axis))
                    .map_or_else(|| region.extent(axis).unwrap(), Range::len);
                (*stride, len)
            })
            .filter(|(_, len)| *len > 1)
            .collect::<Vec<_>>();
        strides.sort_unstable();
        let mut covered = 1usize;
        for (stride, len) in strides {
            if stride > covered {
                return false;
            }
            let Some(next) = stride
                .checked_mul(len - 1)
                .and_then(|span| covered.checked_add(span))
            else {
                return false;
            };
            covered = next;
        }
        true
    });
    if dense {
        if let Some(hull) = window_input_view_for_chunk(view, indices, tile, region, chunk)? {
            let bytes = hull.layout().num_access_bytes();
            return Ok((hull.address_ranges().collect(), bytes));
        }
        return Ok((vec![], 0));
    }
    let mut axes = BTreeSet::new();
    for index in indices {
        match index {
            IndexSpec::Axis(axis) => {
                axes.insert(axis.clone());
            }
            IndexSpec::Affine { affine } => {
                axes.extend(affine.terms.keys().cloned());
            }
            IndexSpec::Zero(_) => {}
        }
    }
    let ranges = axes
        .into_iter()
        .map(|axis| {
            let range = chunk
                .get(&axis)
                .or_else(|| tile.output_intervals.get(&axis))
                .cloned()
                .unwrap_or(0..region.extent(&axis).unwrap());
            (axis, range)
        })
        .collect::<Vec<_>>();
    let mut merged = BTreeMap::<u128, u128>::new();
    let mut coords = BTreeMap::new();
    for_each_coordinate(&ranges, 0, &mut coords, &mut |coords| {
        let mut offsets = view.offsets().to_vec();
        for (dimension, index) in indices.iter().enumerate() {
            if view.shape().dims()[dimension] == 1 {
                continue;
            }
            let position = eval_index(index, coords)?;
            let limit = i128::try_from(view.shape().dims()[dimension])
                .map_err(|_| SimError("window extent overflows".into()))?;
            if position < 0 || position >= limit {
                return Ok(());
            }
            offsets[dimension] = offsets[dimension]
                .checked_add(
                    usize::try_from(position)
                        .map_err(|_| SimError("window offset overflows".into()))?,
                )
                .ok_or_else(|| SimError("window offset overflows".into()))?;
        }
        let element = TensorView::new(view.tensor().clone(), &vec![1; indices.len()], &offsets)?;
        for range in element.address_ranges() {
            let mut start = range.start;
            let mut end = range.end;
            if let Some((&previous_start, &previous_end)) = merged.range(..=start).next_back()
                && previous_end >= start
            {
                start = previous_start;
                end = end.max(previous_end);
                merged.remove(&previous_start);
            }
            while let Some((&next_start, &next_end)) = merged.range(start..).next()
                && next_start <= end
            {
                end = end.max(next_end);
                merged.remove(&next_start);
            }
            merged.insert(start, end);
        }
        Ok(())
    })?;
    let ranges = merged
        .into_iter()
        .map(|(start, end)| start..end)
        .collect::<Vec<_>>();
    let bytes = ranges.iter().try_fold(0usize, |count, range| {
        count
            .checked_add(
                usize::try_from(range.end - range.start)
                    .map_err(|_| SimError("window transfer size overflows".into()))?,
            )
            .ok_or_else(|| SimError("window transfer size overflows".into()))
    })?;
    Ok((ranges, bytes))
}

pub(super) fn window_input_view_for_chunk(
    view: &TensorView,
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ValidatedRegion,
    chunk: &std::collections::BTreeMap<String, Range<usize>>,
) -> Result<Option<TensorView>, SimError> {
    let mut shape = view.shape().dims().to_vec();
    let mut offsets = view.offsets().to_vec();
    for (dimension, index) in indices.iter().enumerate() {
        if shape[dimension] == 1 {
            continue;
        }
        let mut low_coords = std::collections::BTreeMap::new();
        let mut high_coords = std::collections::BTreeMap::new();
        for (axis, extent) in &region.extents {
            let range = chunk
                .get(axis)
                .or_else(|| tile.output_intervals.get(axis))
                .cloned()
                .unwrap_or(0..*extent);
            low_coords.insert(axis.clone(), range.start);
            high_coords.insert(axis.clone(), range.end - 1);
        }
        let minimum = eval_index(index, &low_coords)?.max(0);
        let maximum = eval_index(index, &high_coords)?.min(
            i128::try_from(shape[dimension] - 1)
                .map_err(|_| SimError("window extent overflows".into()))?,
        );
        if minimum > maximum {
            return Ok(None);
        }
        let start =
            usize::try_from(minimum).map_err(|_| SimError("window offset overflows".into()))?;
        let end =
            usize::try_from(maximum + 1).map_err(|_| SimError("window extent overflows".into()))?;
        offsets[dimension] = offsets[dimension]
            .checked_add(start)
            .ok_or_else(|| SimError("window offset overflows".into()))?;
        shape[dimension] = end - start;
    }
    TensorView::new(view.tensor().clone(), &shape, &offsets).map(Some)
}

pub(super) fn window_all_in_range(
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ValidatedRegion,
    input: &TensorView,
) -> Result<bool, SimError> {
    let mut low = std::collections::BTreeMap::new();
    let mut high = std::collections::BTreeMap::new();
    for (axis, extent) in &region.extents {
        let range = tile
            .output_intervals
            .get(axis)
            .cloned()
            .unwrap_or(0..*extent);
        low.insert(axis.clone(), range.start);
        high.insert(axis.clone(), range.end - 1);
    }
    indices
        .iter()
        .zip(input.shape().dims())
        .try_fold(true, |all, (index, extent)| {
            if *extent == 1 {
                return Ok(all);
            }
            let start = eval_index(index, &low)?;
            let end = eval_index(index, &high)?;
            let limit =
                i128::try_from(*extent).map_err(|_| SimError("window extent overflows".into()))?;
            Ok::<bool, SimError>(all && start >= 0 && end < limit)
        })
}

#[expect(clippy::too_many_arguments)]
// Count valid body evaluations and accumulator operations, sharing the first
// valid element across chunks. Excluded operands must all be present.
pub(super) fn count_window_chunk_operations(
    excluded: &[(&[IndexSpec], &TensorView)],
    output_axes: &[String],
    reduction_axes: &[String],
    tile: &RegionTile,
    region: &ValidatedRegion,
    chunk: &std::collections::BTreeMap<String, Range<usize>>,
    first_chunk: bool,
    all_in_range: bool,
    seen: &mut BTreeSet<Vec<usize>>,
) -> Result<(usize, usize), SimError> {
    let outputs = output_axes
        .iter()
        .map(|axis| {
            (
                axis.clone(),
                tile.output_intervals
                    .get(axis)
                    .cloned()
                    .unwrap_or(0..region.extent(axis).unwrap()),
            )
        })
        .collect::<Vec<_>>();
    let reductions = reduction_axes
        .iter()
        .map(|axis| (axis.clone(), chunk[axis].clone()))
        .collect::<Vec<_>>();
    if all_in_range {
        let output_count = outputs.iter().try_fold(1usize, |count, (_, range)| {
            count
                .checked_mul(range.len())
                .ok_or_else(|| SimError("window output count overflows".into()))
        })?;
        let window_count = reductions.iter().try_fold(1usize, |count, (_, range)| {
            count
                .checked_mul(range.len())
                .ok_or_else(|| SimError("window reduction count overflows".into()))
        })?;
        let elements = output_count
            .checked_mul(window_count)
            .ok_or_else(|| SimError("window element count overflows".into()))?;
        let comparisons = output_count
            .checked_mul(window_count - usize::from(first_chunk))
            .ok_or_else(|| SimError("window comparison count overflows".into()))?;
        return Ok((elements, comparisons));
    }
    let mut coords = std::collections::BTreeMap::new();
    let mut comparisons = 0usize;
    let mut elements = 0usize;
    for_each_coordinate(&outputs, 0, &mut coords, &mut |coords| {
        let key = output_axes
            .iter()
            .map(|axis| coords[axis])
            .collect::<Vec<_>>();
        let mut reduction_coords = coords.clone();
        let mut valid = 0usize;
        for_each_coordinate(&reductions, 0, &mut reduction_coords, &mut |coords| {
            let present = excluded
                .iter()
                .try_fold(true, |present, (indices, input)| {
                    indices.iter().zip(input.shape().dims()).try_fold(
                        present,
                        |present, (index, extent)| {
                            if *extent == 1 {
                                return Ok(present);
                            }
                            let value = eval_index(index, coords)?;
                            Ok::<bool, SimError>(
                                present
                                    && value >= 0
                                    && value
                                        < i128::try_from(*extent).map_err(|_| {
                                            SimError("window extent overflows".into())
                                        })?,
                            )
                        },
                    )
                })?;
            if present {
                valid = valid
                    .checked_add(1)
                    .ok_or_else(|| SimError("window valid-element count overflows".into()))?;
            }
            Ok(())
        })?;
        if valid != 0 {
            elements = elements
                .checked_add(valid)
                .ok_or_else(|| SimError("window element count overflows".into()))?;
            let first_valid_chunk = seen.insert(key);
            comparisons = comparisons
                .checked_add(valid - usize::from(first_valid_chunk))
                .ok_or_else(|| SimError("window comparison count overflows".into()))?;
        }
        Ok(())
    })?;
    Ok((elements, comparisons))
}

pub(super) fn for_each_coordinate(
    ranges: &[(String, Range<usize>)],
    position: usize,
    coords: &mut std::collections::BTreeMap<String, usize>,
    visit: &mut impl FnMut(&std::collections::BTreeMap<String, usize>) -> Result<(), SimError>,
) -> Result<(), SimError> {
    if position == ranges.len() {
        return visit(coords);
    }
    let (axis, range) = &ranges[position];
    for value in range.clone() {
        coords.insert(axis.clone(), value);
        for_each_coordinate(ranges, position + 1, coords, visit)?;
    }
    coords.remove(axis);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing_element::program::syntax::{ProgramSpec, TileSpec};
    use crate::processing_element::program::tile::plan_tiles;
    use crate::processing_element::program::validate::{RegionBindings, validate_program};
    use crate::processing_element::tensors::Tensor;
    use crate::processing_element::tensors::dtype::DataType;

    #[test]
    fn window_peak_matches_exhaustive_chunks_for_affine_strides_padding_and_packing() {
        for dtype in [
            DataType::Fp32,
            DataType::Fp16,
            DataType::Int8,
            DataType::Fp4,
            DataType::Bool,
        ] {
            for coefficient in [1, 2, 4] {
                for offset in [-1, 0] {
                    let length = 7 + coefficient * 4;
                    let input = TensorView::new(
                        Tensor::new(&[2, length + 3], &dtype, 0).unwrap(),
                        &[2, length],
                        &[0, 1],
                    )
                    .unwrap();
                    let output = TensorView::new_full(Tensor::new(&[2, 3], &dtype, 4096).unwrap());
                    let program: ProgramSpec = yaml_serde::from_str(&format!(r"
inputs: [A]
outputs: [Y]
extents: {{ y: 3, k: 5 }}
assignments:
 - output: Y
   axes: [b,y]
   expr: {{ reduce: {{ op: sum, over: [k], value: {{ read: {{ tensor: A, indices: [b, {{ affine: {{ terms: {{ y: 3, k: {coefficient} }}, offset: {offset} }} }}], bounds: exclude }} }} }} }}
")).unwrap();
                    let region = validate_program(
                        &program,
                        &RegionBindings {
                            inputs: vec![input.clone()],
                            outputs: vec![output],
                        },
                    )
                    .unwrap();
                    let indices = match &program.assignments[0].expr {
                        crate::processing_element::program::syntax::ExprSpec::Reduce {
                            value,
                            ..
                        } => match &**value {
                            crate::processing_element::program::syntax::ExprSpec::Read {
                                indices,
                                ..
                            } => indices,
                            _ => unreachable!(),
                        },
                        _ => unreachable!(),
                    };
                    for reduction_tile in [1, 2, 3, 5] {
                        let schedule = plan_tiles(
                            &region,
                            Some(&TileSpec {
                                sizes: BTreeMap::from([
                                    ("y".into(), 2),
                                    ("k".into(), reduction_tile),
                                ]),
                            }),
                        )
                        .unwrap();
                        for tile in schedule.tiles() {
                            let over = vec!["k".into()];
                            let exact = tile
                                .reduction_chunks_for(&over)
                                .map(|chunk| {
                                    align8(
                                        window_chunk_access_ranges(
                                            &input, indices, &tile, &region, &chunk,
                                        )
                                        .unwrap()
                                        .1,
                                    )
                                    .unwrap()
                                })
                                .max()
                                .unwrap();
                            let peak = max_window_chunk_buffer_bytes(
                                &input,
                                indices,
                                &tile,
                                &region,
                                &over,
                                usize::MAX,
                            )
                            .unwrap();
                            assert_eq!(
                                peak, exact,
                                "dtype={dtype:?}, coefficient={coefficient}, offset={offset}, tile={reduction_tile}"
                            );
                        }
                    }
                }
            }
        }
    }
}
