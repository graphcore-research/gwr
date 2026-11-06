// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;

use super::eval_index;
use crate::processing_element::program::storage::align_sram_slot_bytes;
use crate::processing_element::program::syntax::IndexSpec;
use crate::processing_element::program::tile::RegionTile;
use crate::processing_element::program::validate::ProgramFacts;
use crate::processing_element::tensors::{HasShape, TensorView};

pub(super) fn max_window_chunk_buffer_bytes(
    view: &TensorView,
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ProgramFacts,
    over: &[String],
    available_sram: usize,
) -> Result<usize, SimError> {
    let mut peak = 0usize;
    let unpadded = window_indices_are_in_range(indices, tile, region, view)?;
    // Full unpadded windows differ only by a coordinate translation; trailing
    // partial chunks are subsets. Retain exhaustive inspection for padding
    // and packed inputs, whose selected-element storage is checked per chunk.
    let chunks = if unpadded && view.dtype().num_bits().is_multiple_of(8) {
        1
    } else {
        usize::MAX
    };
    for chunk in tile.reduction_chunks_for(over).take(chunks) {
        if unpadded {
            let min_bytes = window_chunk_minimum_bytes(view, indices, tile, region, &chunk)?;
            if min_bytes > available_sram {
                return sim_error!(
                    "window tile requires at least {min_bytes} SRAM bytes, available {available_sram}"
                );
            }
        }
        let access = plan_window_chunk_access(view, indices, tile, region, &chunk)?;
        peak = peak.max(align_sram_slot_bytes(access.packed_buffer_bytes)?);
    }
    Ok(peak)
}

fn window_chunk_minimum_bytes(
    view: &TensorView,
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ProgramFacts,
    chunk: &BTreeMap<String, Range<usize>>,
) -> Result<usize, SimError> {
    let elements = indices.iter().zip(view.shape().dims()).try_fold(
        1usize,
        |product, (index, dimension)| {
            if *dimension == 1 {
                return Ok(product);
            }
            product
                .checked_mul(max_varying_axis_length(index, tile, region, chunk))
                .ok_or_else(|| SimError("window SRAM lower bound overflows".into()))
        },
    )?;
    Ok(elements
        .checked_mul(view.dtype().num_bits())
        .ok_or_else(|| SimError("window SRAM lower bound overflows".into()))?
        .div_ceil(8))
}

fn max_varying_axis_length(
    index: &IndexSpec,
    tile: &RegionTile,
    region: &ProgramFacts,
    chunk: &BTreeMap<String, Range<usize>>,
) -> usize {
    let axes = match index {
        IndexSpec::Zero(_) => vec![],
        IndexSpec::Axis(axis) => vec![axis.as_str()],
        IndexSpec::Affine { affine } => affine.terms.keys().map(String::as_str).collect(),
    };
    axes.into_iter()
        .map(|axis| {
            chunk
                .get(axis)
                .or_else(|| tile.output_intervals.get(axis))
                .map_or_else(|| region.extent(axis).unwrap(), Range::len)
        })
        .max()
        .unwrap_or(1)
}

pub(super) struct WindowAccessPlan {
    pub(super) ranges: Vec<Range<u128>>,
    pub(super) transfer_bytes: usize,
    /// Logical packed input storage, independent of physical byte alignment.
    pub(super) packed_buffer_bytes: usize,
}

pub(super) fn plan_window_chunk_access(
    view: &TensorView,
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ProgramFacts,
    chunk: &BTreeMap<String, Range<usize>>,
) -> Result<WindowAccessPlan, SimError> {
    if !is_dense_window(indices, tile, region, chunk) {
        return enumerate_window_access(view, indices, tile, region, chunk);
    }
    let Some(window_input_view) = build_window_input_view(view, indices, tile, region, chunk)?
    else {
        return Ok(WindowAccessPlan {
            ranges: vec![],
            transfer_bytes: 0,
            packed_buffer_bytes: 0,
        });
    };
    Ok(WindowAccessPlan {
        ranges: window_input_view.address_ranges().collect(),
        transfer_bytes: window_input_view.layout().num_access_bytes(),
        packed_buffer_bytes: window_input_view.num_packed_bytes(),
    })
}

fn is_dense_window(
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ProgramFacts,
    chunk: &BTreeMap<String, Range<usize>>,
) -> bool {
    indices.iter().all(|index| {
        let IndexSpec::Affine { affine } = index else {
            return true;
        };
        let strides = affine
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
        strides_cover_dense_range(strides)
    })
}

// Each varying axis must overlap or adjoin the positions already covered.
fn strides_cover_dense_range(mut strides: Vec<(usize, usize)>) -> bool {
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
}

fn enumerate_window_access(
    view: &TensorView,
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ProgramFacts,
    chunk: &BTreeMap<String, Range<usize>>,
) -> Result<WindowAccessPlan, SimError> {
    let ranges = collect_window_coordinate_ranges(indices, tile, region, chunk);
    let mut merged = BTreeMap::<u128, u128>::new();
    let mut packed_elements = BTreeSet::new();
    let mut coords = BTreeMap::new();
    for_each_coordinate(&ranges, 0, &mut coords, &mut |coords| {
        let Some(element) = resolve_window_element(view, indices, coords)? else {
            return Ok(());
        };
        if view.dtype().num_bits() < 8 {
            packed_elements.insert(element.layout().first_element());
        }
        for range in element.address_ranges() {
            merge_address_range(&mut merged, range);
        }
        Ok(())
    })?;
    let ranges = merged
        .into_iter()
        .map(|(start, end)| start..end)
        .collect::<Vec<_>>();
    let bytes = address_range_bytes(&ranges)?;
    let storage_bytes = if view.dtype().num_bits() < 8 {
        packed_elements
            .len()
            .checked_mul(view.dtype().num_bits())
            .ok_or_else(|| SimError("window storage size overflows".into()))?
            .div_ceil(8)
    } else {
        bytes
    };
    Ok(WindowAccessPlan {
        ranges,
        transfer_bytes: bytes,
        packed_buffer_bytes: storage_bytes,
    })
}

fn collect_window_coordinate_ranges(
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ProgramFacts,
    chunk: &BTreeMap<String, Range<usize>>,
) -> Vec<(String, Range<usize>)> {
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
    axes.into_iter()
        .map(|axis| {
            let range = chunk
                .get(&axis)
                .or_else(|| tile.output_intervals.get(&axis))
                .cloned()
                .unwrap_or(0..region.extent(&axis).unwrap());
            (axis, range)
        })
        .collect()
}

fn address_range_bytes(ranges: &[Range<u128>]) -> Result<usize, SimError> {
    ranges.iter().try_fold(0usize, |count, range| {
        count
            .checked_add(
                usize::try_from(range.end - range.start)
                    .map_err(|_| SimError("window transfer size overflows".into()))?,
            )
            .ok_or_else(|| SimError("window transfer size overflows".into()))
    })
}

fn resolve_window_element(
    view: &TensorView,
    indices: &[IndexSpec],
    coords: &BTreeMap<String, usize>,
) -> Result<Option<TensorView>, SimError> {
    let mut offsets = view.offsets().to_vec();
    for (dimension, index) in indices.iter().enumerate() {
        if view.shape().dims()[dimension] == 1 {
            continue;
        }
        let position = eval_index(index, coords)?;
        let limit = i128::try_from(view.shape().dims()[dimension])
            .map_err(|_| SimError("window extent overflows".into()))?;
        if position < 0 || position >= limit {
            return Ok(None);
        }
        offsets[dimension] = offsets[dimension]
            .checked_add(
                usize::try_from(position)
                    .map_err(|_| SimError("window offset overflows".into()))?,
            )
            .ok_or_else(|| SimError("window offset overflows".into()))?;
    }
    TensorView::new(view.tensor().clone(), &vec![1; indices.len()], &offsets).map(Some)
}

fn merge_address_range(merged: &mut BTreeMap<u128, u128>, range: Range<u128>) {
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

pub(super) fn build_window_input_view(
    view: &TensorView,
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ProgramFacts,
    chunk: &BTreeMap<String, Range<usize>>,
) -> Result<Option<TensorView>, SimError> {
    let mut shape = view.shape().dims().to_vec();
    let mut offsets = view.offsets().to_vec();
    let (low_coords, high_coords) = build_window_coordinate_corners(tile, region, chunk);
    for (dimension, index) in indices.iter().enumerate() {
        if shape[dimension] == 1 {
            continue;
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

pub(super) fn window_indices_are_in_range(
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ProgramFacts,
    input: &TensorView,
) -> Result<bool, SimError> {
    let (low, high) = build_window_coordinate_corners(tile, region, &BTreeMap::new());
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

fn build_window_coordinate_corners(
    tile: &RegionTile,
    region: &ProgramFacts,
    chunk: &BTreeMap<String, Range<usize>>,
) -> (BTreeMap<String, usize>, BTreeMap<String, usize>) {
    let mut low_coords = BTreeMap::new();
    let mut high_coords = BTreeMap::new();
    for (axis, extent) in &region.extents {
        let range = chunk
            .get(axis)
            .or_else(|| tile.output_intervals.get(axis))
            .cloned()
            .unwrap_or(0..*extent);
        low_coords.insert(axis.clone(), range.start);
        high_coords.insert(axis.clone(), range.end - 1);
    }
    (low_coords, high_coords)
}

#[expect(clippy::too_many_arguments)]
// Count valid body evaluations and accumulator operations, sharing the first
// valid element across chunks. Excluded operands must all be present.
pub(super) fn count_window_chunk_operations(
    excluded: &[(&[IndexSpec], &TensorView)],
    output_axes: &[String],
    reduction_axes: &[String],
    tile: &RegionTile,
    region: &ProgramFacts,
    chunk: &BTreeMap<String, Range<usize>>,
    first_chunk: bool,
    all_in_range: bool,
    seen: &mut BTreeSet<Vec<usize>>,
) -> Result<(usize, usize), SimError> {
    let output_ranges = output_axes
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
    let reduction_ranges = reduction_axes
        .iter()
        .map(|axis| (axis.clone(), chunk[axis].clone()))
        .collect::<Vec<_>>();
    if all_in_range {
        count_in_range_window_operations(&output_ranges, &reduction_ranges, first_chunk)
    } else {
        count_window_operations_with_exclusions(excluded, &output_ranges, &reduction_ranges, seen)
    }
}

fn count_in_range_window_operations(
    output_ranges: &[(String, Range<usize>)],
    reduction_ranges: &[(String, Range<usize>)],
    first_chunk: bool,
) -> Result<(usize, usize), SimError> {
    let output_count =
        count_coordinate_combinations(output_ranges, "window output count overflows")?;
    let window_count =
        count_coordinate_combinations(reduction_ranges, "window reduction count overflows")?;
    let elements = output_count
        .checked_mul(window_count)
        .ok_or_else(|| SimError("window element count overflows".into()))?;

    // Each output's first element initializes its accumulator.
    let operations_per_output = window_count - usize::from(first_chunk);
    let accumulator_operations = output_count
        .checked_mul(operations_per_output)
        .ok_or_else(|| SimError("window comparison count overflows".into()))?;
    Ok((elements, accumulator_operations))
}

fn count_coordinate_combinations(
    ranges: &[(String, Range<usize>)],
    overflow_message: &str,
) -> Result<usize, SimError> {
    ranges.iter().try_fold(1usize, |count, (_, range)| {
        count
            .checked_mul(range.len())
            .ok_or_else(|| SimError(overflow_message.into()))
    })
}

fn count_window_operations_with_exclusions(
    excluded: &[(&[IndexSpec], &TensorView)],
    output_ranges: &[(String, Range<usize>)],
    reduction_ranges: &[(String, Range<usize>)],
    seen_outputs: &mut BTreeSet<Vec<usize>>,
) -> Result<(usize, usize), SimError> {
    let mut coords = BTreeMap::new();
    let mut accumulator_operations = 0usize;
    let mut elements = 0usize;
    for_each_coordinate(output_ranges, 0, &mut coords, &mut |coords| {
        let valid = count_valid_window_elements(excluded, reduction_ranges, coords)?;
        if valid == 0 {
            return Ok(());
        }
        elements = elements
            .checked_add(valid)
            .ok_or_else(|| SimError("window element count overflows".into()))?;

        // Excluded chunks do not initialize the accumulator. Remember the first
        // nonempty chunk separately for each output position.
        let key = output_ranges
            .iter()
            .map(|(axis, _)| coords[axis])
            .collect::<Vec<_>>();
        let first_valid_chunk = seen_outputs.insert(key);
        accumulator_operations = accumulator_operations
            .checked_add(valid - usize::from(first_valid_chunk))
            .ok_or_else(|| SimError("window comparison count overflows".into()))?;
        Ok(())
    })?;
    Ok((elements, accumulator_operations))
}

fn count_valid_window_elements(
    excluded: &[(&[IndexSpec], &TensorView)],
    reduction_ranges: &[(String, Range<usize>)],
    output_coords: &BTreeMap<String, usize>,
) -> Result<usize, SimError> {
    let mut coords = output_coords.clone();
    let mut valid = 0usize;
    for_each_coordinate(reduction_ranges, 0, &mut coords, &mut |coords| {
        if window_inputs_are_in_range(excluded, coords)? {
            valid = valid
                .checked_add(1)
                .ok_or_else(|| SimError("window valid-element count overflows".into()))?;
        }
        Ok(())
    })?;
    Ok(valid)
}

fn window_inputs_are_in_range(
    inputs: &[(&[IndexSpec], &TensorView)],
    coords: &BTreeMap<String, usize>,
) -> Result<bool, SimError> {
    let mut present = true;
    for (indices, input) in inputs {
        for (index, extent) in indices.iter().zip(input.shape().dims()) {
            // Singleton dimensions broadcast over their logical indices.
            if *extent == 1 {
                continue;
            }
            let value = eval_index(index, coords)?;
            present = present
                && value >= 0
                && value
                    < i128::try_from(*extent)
                        .map_err(|_| SimError("window extent overflows".into()))?;
        }
    }
    Ok(present)
}

pub(super) fn for_each_coordinate(
    ranges: &[(String, Range<usize>)],
    position: usize,
    coords: &mut BTreeMap<String, usize>,
    visit: &mut impl FnMut(&BTreeMap<String, usize>) -> Result<(), SimError>,
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
    use yaml_serde::from_str;

    use super::*;
    use crate::processing_element::program::syntax::{ExprSpec, ProgramSpec, TileSpec};
    use crate::processing_element::program::tile::plan_tiles;
    use crate::processing_element::program::validate::{RegionBindings, validate_program};
    use crate::processing_element::tensors::Tensor;
    use crate::processing_element::tensors::dtype::DataType;

    fn window_region(
        dtype: DataType,
        coefficient: usize,
        offset: i32,
    ) -> (ProgramSpec, TensorView, ProgramFacts) {
        let length = 7 + coefficient * 4;
        let input = TensorView::new(
            Tensor::new(&[2, length + 3], &dtype, 0).unwrap(),
            &[2, length],
            &[0, 1],
        )
        .unwrap();
        let output = TensorView::new_full(Tensor::new(&[2, 3], &dtype, 4096).unwrap());
        let program = window_program(coefficient, offset);
        let region = validate_program(
            &program,
            RegionBindings {
                inputs: vec![input.clone()],
                outputs: vec![output],
            },
        )
        .unwrap();
        (program, input, region)
    }

    fn window_program(coefficient: usize, offset: i32) -> ProgramSpec {
        from_str(&format!(
            r"
inputs: [A]
outputs: [Y]
extents: {{ y: 3, k: 5 }}
assignments:
 - output: Y
   axes: [b,y]
   expr:
     reduce:
       op: sum
       over: [k]
       value:
         read:
           tensor: A
           indices:
             - b
             - affine:
                 terms: {{ y: 3, k: {coefficient} }}
                 offset: {offset}
           bounds: exclude
"
        ))
        .unwrap()
    }

    // Independently enumerate logical coordinates, resolve each input identity,
    // and pack the unique selected elements. Repeated overlapping-window reads
    // share storage; byte addresses and planner helpers are deliberately
    // unused.
    fn selected_element_storage_bytes(
        input: &TensorView,
        indices: &[IndexSpec],
        tile: &RegionTile,
        region: &ProgramFacts,
        chunk: &BTreeMap<String, Range<usize>>,
    ) -> usize {
        let axes = region.extents.keys().collect::<Vec<_>>();
        let ranges = axes
            .iter()
            .map(|axis| {
                chunk
                    .get(*axis)
                    .or_else(|| tile.output_intervals.get(*axis))
                    .cloned()
                    .unwrap_or(0..region.extents[*axis])
            })
            .collect::<Vec<_>>();
        let combinations = ranges.iter().map(Range::len).product();
        let mut selected = BTreeSet::new();
        for mut ordinal in 0..combinations {
            let mut coords = BTreeMap::new();
            for (axis, range) in axes.iter().zip(&ranges) {
                coords.insert(axis.as_str(), range.start + ordinal % range.len());
                ordinal /= range.len();
            }
            let mut identity = Vec::new();
            for (dimension, index) in indices.iter().enumerate() {
                let extent = input.shape().dims()[dimension];
                let position = if extent == 1 {
                    0
                } else {
                    match index {
                        IndexSpec::Zero(_) => 0,
                        IndexSpec::Axis(axis) => coords[axis.as_str()] as i128,
                        IndexSpec::Affine { affine } => {
                            i128::from(affine.offset)
                                + affine
                                    .terms
                                    .iter()
                                    .map(|(axis, stride)| {
                                        coords[axis.as_str()] as i128 * *stride as i128
                                    })
                                    .sum::<i128>()
                        }
                    }
                };
                if position < 0 || position >= extent as i128 {
                    break;
                }
                identity.push(input.offsets()[dimension] + position as usize);
            }
            if identity.len() == indices.len() {
                selected.insert(identity);
            }
        }
        (selected.len() * input.dtype().num_bits()).div_ceil(8)
    }

    #[test]
    fn sparse_packed_reductions_store_selected_elements_without_transfer_gaps() {
        for (dtype, stride, packed_bytes) in [(DataType::Int4, 2, 8), (DataType::Bool, 8, 2)] {
            let program: ProgramSpec = from_str(&format!(
                r"
inputs: [A]
outputs: [Y]
extents: {{ k: 16 }}
assignments:
 - output: Y
   axes: []
   expr:
     reduce:
       op: sum
       over: [k]
       value:
         read:
           tensor: A
           indices: [{{ affine: {{ terms: {{ k: {stride} }}, offset: 0 }} }}]
"
            ))
            .unwrap();
            let input = TensorView::new_full(Tensor::new(&[16 * stride], &dtype, 0).unwrap());
            let region = validate_program(
                &program,
                RegionBindings {
                    inputs: vec![input.clone()],
                    outputs: vec![TensorView::new_full(
                        Tensor::new(&[], &dtype, 4096).unwrap(),
                    )],
                },
            )
            .unwrap();
            let ExprSpec::Read { indices, .. } =
                program.assignments[0].expr.iter_reads().next().unwrap()
            else {
                unreachable!()
            };
            let tile = plan_tiles(&region, None).unwrap().tiles().next().unwrap();
            let over = vec!["k".into()];
            let chunk = tile.reduction_chunks_for(&over).next().unwrap();
            let access = plan_window_chunk_access(&input, indices, &tile, &region, &chunk).unwrap();
            assert_eq!(access.ranges, vec![0..16]);
            assert_eq!(access.transfer_bytes, 16);
            assert_eq!(access.packed_buffer_bytes, packed_bytes);
            assert_eq!(
                selected_element_storage_bytes(&input, indices, &tile, &region, &chunk),
                packed_bytes
            );
            assert_eq!(
                max_window_chunk_buffer_bytes(&input, indices, &tile, &region, &over, 8).unwrap(),
                8,
                "{dtype:?}"
            );
        }
    }

    #[test]
    fn window_peak_matches_exhaustive_chunks_for_affine_strides_padding_and_packing() {
        for dtype in [
            DataType::Fp32,
            DataType::Fp16,
            DataType::Int8,
            DataType::Fp4,
            DataType::Int4,
            DataType::Bool,
        ] {
            for coefficient in [1, 2, 4] {
                for offset in [-1, 0] {
                    let (program, input, region) = window_region(dtype, coefficient, offset);
                    let ExprSpec::Read { indices, .. } =
                        program.assignments[0].expr.iter_reads().next().unwrap()
                    else {
                        unreachable!()
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
                                    selected_element_storage_bytes(
                                        &input, indices, &tile, &region, &chunk,
                                    )
                                    .next_multiple_of(8)
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
