// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Uncompiled proposal: caller Uint8 scalar scale-plane boundary ranges.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use gwr_engine::types::SimError;

use super::syntax::{BoundsMode, ExprSpec, IndexSpec, ProgramSpec};
use super::tile::RegionTile;
use super::validate::{RegionBindings, ValidatedRegion};
use crate::processing_element::tensors::dtype::DataType;
use crate::processing_element::tensors::{HasShape, TensorView};

fn err(message: impl Into<String>) -> SimError {
    SimError(message.into())
}

fn collect(
    expr: &ExprSpec,
    name: &str,
    allowed: bool,
    reads: &mut Vec<(String, usize)>,
) -> Result<(), SimError> {
    match expr {
        ExprSpec::Read {
            tensor,
            indices,
            bounds,
        } if tensor == name => {
            if !allowed || *bounds != BoundsMode::RequireInRange {
                return Err(err("indexed metadata supports only in-range scalar reads"));
            }
            let [IndexSpec::Axis(axis), IndexSpec::Affine { affine }] = indices.as_slice() else {
                return Err(err(
                    "indexed metadata requires [output axis, constant affine column]",
                ));
            };
            if !affine.terms.is_empty() || affine.offset < 0 {
                return Err(err(
                    "indexed metadata column must be a nonnegative constant",
                ));
            }
            let column = usize::try_from(affine.offset)
                .map_err(|_| err("indexed metadata column overflows"))?;
            reads.push((axis.clone(), column));
        }
        ExprSpec::Call { args, .. } => {
            for arg in args {
                collect(arg, name, allowed, reads)?;
            }
        }
        ExprSpec::MatrixMac {
            args, quantization, ..
        } => {
            if quantization.as_ref().is_some_and(|formats| {
                formats.iter().any(|format| {
                    format.block_scales == name || format.global_scale.as_deref() == Some(name)
                })
            }) {
                return Err(err(
                    "native matrix quantization metadata must retain full-view loads",
                ));
            }
            for arg in args {
                collect(arg, name, false, reads)?;
            }
        }
        ExprSpec::Reduce { value, .. } => collect(value, name, false, reads)?,
        ExprSpec::Read { .. } => {}
    }
    Ok(())
}

fn declaration(
    spec: &ProgramSpec,
    bindings: &RegionBindings,
    extents: &BTreeMap<String, usize>,
    name: &str,
) -> Result<(String, BTreeSet<usize>), SimError> {
    let index = spec
        .inputs
        .iter()
        .position(|input| input == name)
        .ok_or_else(|| err("indexed metadata must bind an input"))?;
    let view = &bindings.inputs[index];
    if view.dtype() != &DataType::Uint8 || view.num_dims() != 2 {
        return Err(err("indexed metadata requires a rank-two Uint8 plane"));
    }
    let mut reads = Vec::new();
    for assignment in &spec.assignments {
        collect(&assignment.expr, name, true, &mut reads)?;
    }
    let Some((axis, _)) = reads.first() else {
        return Err(err("indexed metadata requires explicit scalar reads"));
    };
    if !spec.assignments.iter().any(|assignment| {
        spec.outputs.contains(&assignment.primary_result()) && assignment.axes.contains(axis)
    }) {
        return Err(err("indexed metadata row must be an output tile axis"));
    }
    let rows = extents
        .get(axis)
        .copied()
        .ok_or_else(|| err("indexed metadata row extent missing"))?;
    if rows != view.shape().dims()[0] || reads.iter().any(|(other, _)| other != axis) {
        return Err(err(
            "indexed metadata row axis must cover its complete bound view",
        ));
    }
    let columns = reads
        .iter()
        .map(|(_, column)| *column)
        .collect::<BTreeSet<_>>();
    if columns
        .iter()
        .any(|column| *column >= view.shape().dims()[1])
    {
        return Err(err("indexed metadata column is outside its bound view"));
    }
    rows.checked_mul(columns.len())
        .ok_or_else(|| err("indexed metadata payload overflows"))?;
    Ok((axis.clone(), columns))
}

/// Called by ordinary validation before tile admission, reservation or LSU.
pub(super) fn validate(
    spec: &ProgramSpec,
    bindings: &RegionBindings,
    extents: &BTreeMap<String, usize>,
) -> Result<(), SimError> {
    let names = spec.indexed_metadata_inputs.iter().collect::<BTreeSet<_>>();
    if names.len() != spec.indexed_metadata_inputs.len()
        || names
            .iter()
            .any(|name| !spec.metadata_inputs.contains(name))
    {
        return Err(err(
            "indexed_metadata_inputs must be a distinct subset of metadata_inputs",
        ));
    }
    for name in names {
        declaration(spec, bindings, extents, name)?;
    }
    Ok(())
}

/// One coalesced union of all explicit scalar reads for this output tile.
/// Uses actual TensorView parent offsets/strides, never visible-width
/// arithmetic.
pub(crate) fn ranges(
    region: &ValidatedRegion,
    tile: &RegionTile,
    name: &str,
) -> Result<(Vec<Range<u128>>, usize), SimError> {
    let (axis, columns) = declaration(&region.program, &region.bindings, &region.extents, name)?;
    let rows = tile
        .output_intervals
        .get(&axis)
        .ok_or_else(|| err("indexed metadata tile row interval missing"))?;
    let index = region
        .program
        .inputs
        .iter()
        .position(|input| input == name)
        .unwrap();
    let view = &region.bindings.inputs[index];
    if rows.start >= rows.end || rows.end > view.shape().dims()[0] {
        return Err(err(
            "indexed metadata tile row interval is outside its bound view",
        ));
    }
    let mut all = Vec::new();
    for column in columns {
        let offsets = [
            view.offsets()[0]
                .checked_add(rows.start)
                .ok_or_else(|| err("indexed metadata row offset overflows"))?,
            view.offsets()[1]
                .checked_add(column)
                .ok_or_else(|| err("indexed metadata column offset overflows"))?,
        ];
        let slice = TensorView::new(view.tensor().clone(), &[rows.end - rows.start, 1], &offsets)?;
        all.extend(slice.address_ranges());
    }
    all.sort_by_key(|range| range.start);
    let mut union: Vec<Range<u128>> = Vec::new();
    for range in all {
        if let Some(last) = union.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
            continue;
        }
        union.push(range);
    }
    let bytes = union.iter().try_fold(0usize, |total, range| {
        let size = usize::try_from(range.end - range.start)
            .map_err(|_| err("indexed metadata range length overflows"))?;
        total
            .checked_add(size)
            .ok_or_else(|| err("indexed metadata payload overflows"))
    })?;
    Ok((union, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing_element::program::syntax::{
        PackedMatrixFormat, ScaleLayout, ScaledOperandSpec, TileSpec,
    };
    use crate::processing_element::program::tile::plan_tiles;
    use crate::processing_element::program::validate::validate_program;
    use crate::processing_element::tensors::Tensor;

    const PROGRAM: &str = "
inputs: [S]
metadata_inputs: [S]
indexed_metadata_inputs: [S]
outputs: [Y]
temporaries: {U: {dtype: uint8}, V: {dtype: uint8}, W: {dtype: uint8}, D: {dtype: uint8}}
assignments:
- output: U
  axes: [n]
  expr: {read: {tensor: S, indices: [n, {affine: {terms: {}, offset: 1}}]}}
- output: V
  axes: [n]
  expr: {read: {tensor: S, indices: [n, {affine: {terms: {}, offset: 0}}]}}
- output: W
  axes: [n]
  expr: {read: {tensor: S, indices: [n, {affine: {terms: {}, offset: 2}}]}}
- output: D
  axes: [n]
  expr: {read: {tensor: S, indices: [n, {affine: {terms: {}, offset: 0}}]}}
- output: Y
  axes: [n]
  expr:
    add:
    - {read: {tensor: U, indices: [n]}}
    - add:
      - {read: {tensor: V, indices: [n]}}
      - add:
        - {read: {tensor: W, indices: [n]}}
        - {read: {tensor: D, indices: [n]}}
";

    fn fixture() -> (ProgramSpec, RegionBindings) {
        let spec = yaml_serde::from_str(PROGRAM).unwrap();
        let parent = Tensor::new(&[8, 5], &DataType::Uint8, 100).unwrap();
        let input = TensorView::new(parent, &[4, 3], &[1, 1]).unwrap();
        let output = TensorView::new_full(Tensor::new(&[4], &DataType::Uint8, 300).unwrap());
        (
            spec,
            RegionBindings {
                inputs: vec![input],
                outputs: vec![output],
            },
        )
    }

    #[test]
    fn all_assignment_union_uses_parent_stride_and_offsets() {
        let (spec, bindings) = fixture();
        let region = validate_program(&spec, &bindings).unwrap();
        let tiling = TileSpec {
            sizes: BTreeMap::from([("n".into(), 2)]),
        };
        let tiles = plan_tiles(&region, Some(&tiling))
            .unwrap()
            .tiles()
            .collect::<Vec<_>>();
        assert_eq!(tiles.len(), 2);
        assert_eq!(
            ranges(&region, &tiles[0], "S").unwrap(),
            (vec![106..109, 111..114], 6)
        );
        assert_eq!(
            ranges(&region, &tiles[1], "S").unwrap(),
            (vec![116..119, 121..124], 6)
        );
        assert_eq!(bindings.inputs[0].num_packed_bytes(), 12);
        assert_eq!(bindings.inputs[0].tensor().num_bytes(), 40);
    }

    #[test]
    fn omitted_policy_keeps_exact_serialized_bytes() {
        let (mut spec, bindings) = fixture();
        spec.indexed_metadata_inputs.clear();
        let bytes = yaml_serde::to_string(&spec).unwrap();
        assert!(!bytes.contains("indexed_metadata_inputs"));
        let restored: ProgramSpec = yaml_serde::from_str(&bytes).unwrap();
        assert_eq!(restored, spec);
        assert_eq!(yaml_serde::to_string(&restored).unwrap(), bytes);
        validate_program(&restored, &bindings).unwrap();
    }

    #[test]
    fn valid_first_header_and_column_controls_reject_intended_categories() {
        let (spec, bindings) = fixture();
        validate_program(&spec, &bindings).unwrap();
        let mut duplicate = spec.clone();
        duplicate.indexed_metadata_inputs.push("S".into());
        assert!(
            validate_program(&duplicate, &bindings)
                .unwrap_err()
                .0
                .contains("distinct subset")
        );
        let mut absent = spec.clone();
        absent.indexed_metadata_inputs = vec!["Unknown".into()];
        assert!(
            validate_program(&absent, &bindings)
                .unwrap_err()
                .0
                .contains("distinct subset")
        );
        let mut bad = spec.clone();
        let ExprSpec::Read { indices, .. } = &mut bad.assignments[0].expr else {
            unreachable!()
        };
        let IndexSpec::Affine { affine } = &mut indices[1] else {
            unreachable!()
        };
        affine.offset = 3;
        assert!(validate_program(&bad, &bindings).is_err());
        let wrong = RegionBindings {
            inputs: vec![TensorView::new_full(
                Tensor::new(&[4, 3], &DataType::Int8, 100).unwrap(),
            )],
            outputs: bindings.outputs.clone(),
        };
        assert!(
            validate(&spec, &wrong, &BTreeMap::from([("n".into(), 4)]))
                .unwrap_err()
                .0
                .contains("Uint8")
        );
    }

    #[test]
    fn nested_implicit_native_scale_and_reduced_reads_retain_full_view() {
        let (spec, bindings) = fixture();
        validate_program(&spec, &bindings).unwrap();
        let scale = ScaledOperandSpec {
            format: PackedMatrixFormat::Mxfp4,
            block_size: 32,
            block_scales: "S".into(),
            global_scale: None,
            layout: ScaleLayout::Logical,
        };
        let native = ExprSpec::MatrixMac {
            execution: None,
            args: vec![],
            over: vec!["k".into()],
            quantization: Some([scale.clone(), scale]),
        };
        let nested = ExprSpec::Call {
            op: "convert".into(),
            args: vec![native],
            attrs: BTreeMap::new(),
        };
        assert!(
            collect(&nested, "S", true, &mut vec![])
                .unwrap_err()
                .0
                .contains("native matrix")
        );
        let reduced = ExprSpec::Reduce {
            op: super::super::syntax::ReductionOp::Sum,
            over: vec!["n".into()],
            value: Box::new(spec.assignments[0].expr.clone()),
        };
        assert!(
            collect(&reduced, "S", true, &mut vec![])
                .unwrap_err()
                .0
                .contains("scalar reads")
        );
    }
}
