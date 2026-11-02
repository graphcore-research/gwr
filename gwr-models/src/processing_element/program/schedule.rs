// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Infer ordered tile groups and storage scopes from output axes and
//! dependencies.

use std::collections::{BTreeMap, BTreeSet};

use gwr_engine::types::SimError;

use super::syntax::{ExprSpec, TileSpec};
use super::validate::ValidatedRegion;

pub(crate) fn plan_schedule(
    region: &ValidatedRegion,
    tiling: Option<&TileSpec>,
) -> Result<Option<ScopedSchedule>, SimError> {
    validate_sizes(region, tiling)?;
    if !needs_phased_schedule(region, tiling) {
        return Ok(None);
    }
    let common = find_common_output_axes(region);
    let mut plan = ScopedSchedule {
        tiles: resolve_axis_tile_sizes(&common, region, tiling),
        steps: Vec::new(),
        local_values: Vec::new(),
    };
    let mut locations = BTreeMap::new();
    for (axes, assignments) in plan_assignment_groups(region, &common) {
        let position = plan.steps.len();
        let path = if axes.is_empty() {
            vec![position]
        } else {
            vec![position, 0]
        };
        for &assignment in &assignments {
            locations.insert(assignment, path.clone());
        }
        let step = ScopedStep::Group(assignments);
        plan.steps.push(if axes.is_empty() {
            step
        } else {
            ScopedStep::Tiles(ScopedSchedule {
                tiles: resolve_axis_tile_sizes(&axes, region, tiling),
                steps: vec![step],
                local_values: Vec::new(),
            })
        });
    }
    retain_cross_group_values(&mut plan, &locations, region);
    Ok(Some(plan))
}

fn needs_phased_schedule(region: &ValidatedRegion, tiling: Option<&TileSpec>) -> bool {
    tiling.is_some_and(|tiling| {
        tiling.sizes.iter().any(|(axis, step)| {
            *step < region.extents[axis]
                && region
                    .program
                    .assignments
                    .iter()
                    .any(|assignment| assignment.axes.contains(axis))
                && region
                    .program
                    .assignments
                    .iter()
                    .any(|assignment| !assignment.axes.contains(axis))
        })
    })
}

fn find_common_output_axes(region: &ValidatedRegion) -> BTreeSet<String> {
    let first = &region.program.assignments[region.assignment_order[0]];
    first
        .axes
        .iter()
        .filter(|axis| {
            region
                .program
                .assignments
                .iter()
                .all(|assignment| assignment.axes.contains(axis))
        })
        .cloned()
        .collect::<BTreeSet<_>>()
}

/// Continue the current tile group whenever a matching assignment is ready.
/// Otherwise start with the ready group having the fewest free output axes,
/// keeping the validated topological order as the deterministic tie breaker.
fn plan_assignment_groups(
    region: &ValidatedRegion,
    common: &BTreeSet<String>,
) -> Vec<(BTreeSet<String>, Vec<usize>)> {
    let axes = collect_assignment_group_axes(region, common);
    let dependencies = build_assignment_dependency_sets(region);
    let mut pending = region.assignment_order.clone();
    let mut done = BTreeSet::new();
    let mut groups = Vec::<(BTreeSet<String>, Vec<usize>)>::new();
    while !pending.is_empty() {
        let current = groups.last().map(|(axes, _)| axes);
        let index = pending
            .iter()
            .copied()
            .filter(|index| dependencies[*index].is_subset(&done))
            .min_by_key(|index| (current != Some(&axes[*index]), axes[*index].len()))
            .expect("validated assignment dependencies are acyclic");
        if let Some((_, assignments)) = groups
            .last_mut()
            .filter(|(group_axes, _)| group_axes == &axes[index])
        {
            assignments.push(index);
        } else {
            groups.push((axes[index].clone(), vec![index]));
        }
        done.insert(index);
        pending.retain(|pending| *pending != index);
    }
    groups
}

fn collect_assignment_group_axes(
    region: &ValidatedRegion,
    common: &BTreeSet<String>,
) -> Vec<BTreeSet<String>> {
    region
        .program
        .assignments
        .iter()
        .map(|assignment| {
            assignment
                .axes
                .iter()
                .filter(|axis| !common.contains(*axis))
                .cloned()
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>()
}

fn build_assignment_dependency_sets(region: &ValidatedRegion) -> Vec<BTreeSet<usize>> {
    let producers = region
        .program
        .assignments
        .iter()
        .enumerate()
        .flat_map(|(index, assignment)| {
            assignment
                .output
                .iter()
                .chain(assignment.indices_output.iter())
                .map(move |name| (name.clone(), index))
        })
        .collect::<BTreeMap<_, _>>();
    region
        .program
        .assignments
        .iter()
        .map(|assignment| {
            assignment
                .expr
                .collect_read_tensor_names()
                .iter()
                .filter_map(|name| producers.get(name).copied())
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>()
}

fn validate_sizes(region: &ValidatedRegion, tiling: Option<&TileSpec>) -> Result<(), SimError> {
    let Some(tiling) = tiling else {
        return Ok(());
    };
    let mut known = BTreeSet::new();
    for assignment in &region.program.assignments {
        known.extend(assignment.axes.iter().cloned());
        for expr in assignment.expr.walk() {
            if let ExprSpec::Reduce { over, .. } | ExprSpec::MatrixMac { over, .. } = expr {
                known.extend(over.iter().cloned());
            }
        }
    }
    for (axis, step) in &tiling.sizes {
        if !known.contains(axis) {
            return Err(SimError(format!("unknown tile axis {axis}")));
        }
        if *step == 0 {
            return Err(SimError(format!("tile size for {axis} must be positive")));
        }
    }
    Ok(())
}

fn resolve_axis_tile_sizes(
    axes: &BTreeSet<String>,
    region: &ValidatedRegion,
    tiling: Option<&TileSpec>,
) -> BTreeMap<String, usize> {
    axes.iter()
        .map(|axis| {
            (
                axis.clone(),
                tiling
                    .and_then(|tiling| tiling.sizes.get(axis))
                    .copied()
                    .unwrap_or(region.extents[axis]),
            )
        })
        .collect()
}

fn retain_cross_group_values(
    plan: &mut ScopedSchedule,
    locations: &BTreeMap<usize, Vec<usize>>,
    region: &ValidatedRegion,
) {
    for (producer, assignment) in region.program.assignments.iter().enumerate() {
        for name in assignment
            .output
            .iter()
            .chain(assignment.indices_output.iter())
        {
            let consumers = region
                .program
                .assignments
                .iter()
                .enumerate()
                .filter(|(_, consumer)| {
                    consumer
                        .expr
                        .iter_read_tensor_names()
                        .any(|read| read == name.as_str())
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if let Some(owner) = find_retained_value_scope_path(producer, &consumers, locations) {
                scope_at_path_mut(plan, &owner)
                    .local_values
                    .push(name.clone());
            }
        }
    }
}

fn find_retained_value_scope_path(
    producer: usize,
    consumers: &[usize],
    locations: &BTreeMap<usize, Vec<usize>>,
) -> Option<Vec<usize>> {
    if !consumers
        .iter()
        .any(|consumer| locations[consumer] != locations[&producer])
    {
        return None;
    }
    let mut owner = locations[&producer].clone();
    owner.pop(); // Assignment location ends with the containing group.
    for consumer in consumers {
        let path = &locations[consumer];
        owner.truncate(owner.iter().zip(path).take_while(|(a, b)| a == b).count());
    }
    Some(owner)
}

fn scope_at_path_mut<'a>(
    mut scope: &'a mut ScopedSchedule,
    path: &[usize],
) -> &'a mut ScopedSchedule {
    for &step in path {
        let ScopedStep::Tiles(child) = &mut scope.steps[step] else {
            unreachable!();
        };
        scope = child;
    }
    scope
}

#[derive(Clone, Debug)]
pub(crate) struct ScopedSchedule {
    pub tiles: BTreeMap<String, usize>,
    pub steps: Vec<ScopedStep>,
    pub local_values: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum ScopedStep {
    Group(Vec<usize>),
    Tiles(ScopedSchedule),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use yaml_serde::from_str;

    use super::super::plan::allocate_temporaries;
    use super::super::tile::{RegionTile, plan_tiles, validate_tiling};
    use super::{ScopedStep, plan_schedule};
    use crate::processing_element::program::syntax::{ProgramSpec, TileSpec};
    use crate::processing_element::program::validate::{
        RegionBindings, ValidatedRegion, validate_program,
    };
    use crate::processing_element::tensors::dtype::DataType;
    use crate::processing_element::tensors::{Tensor, TensorView};

    fn row_region() -> ValidatedRegion {
        let program: ProgramSpec = from_str(
            r"
inputs: [A]
outputs: [Y]
temporaries: { M: {dtype: fp32}, S: {dtype: fp32} }
assignments:
  - output: M
    axes: [r]
    expr:
      reduce:
        op: max
        over: [k]
        value:
          read: { tensor: A, indices: [r, k] }
  - output: S
    axes: [r]
    expr:
      add:
        - read: { tensor: M, indices: [r] }
        - read: { tensor: M, indices: [r] }
  - output: Y
    axes: [r, k]
    expr:
      add:
        - read: { tensor: A, indices: [r, k] }
        - read: { tensor: S, indices: [r] }
",
        )
        .unwrap();
        let view =
            |addr| TensorView::new_full(Tensor::new(&[3, 9], &DataType::Fp32, addr).unwrap());
        validate_program(
            &program,
            &RegionBindings {
                inputs: vec![view(0)],
                outputs: vec![view(512)],
            },
        )
        .unwrap()
    }

    fn row_tiling() -> TileSpec {
        TileSpec {
            sizes: BTreeMap::from([("r".into(), 1), ("k".into(), 4)]),
        }
    }
    fn pipeline_with_statistic(statistic: &str) -> ValidatedRegion {
        let statistic = statistic.trim().replace('\n', "\n     ");
        let program: ProgramSpec = from_str(&format!(
            r"
inputs: [A, B]
outputs: [S, Y]
temporaries: {{T: {{dtype: fp32}}}}
assignments:
 - output: T
   axes: [i]
   expr:
     read: {{ tensor: A, indices: [i] }}
 - output: S
   axes: []
   expr:
     {statistic}
 - output: Y
   axes: [i]
   expr:
     read: {{ tensor: T, indices: [i] }}
"
        ))
        .unwrap();
        let view = |shape: &[usize], addr| {
            TensorView::new_full(Tensor::new(shape, &DataType::Fp32, addr).unwrap())
        };
        validate_program(
            &program,
            &RegionBindings {
                inputs: vec![view(&[9], 0), view(&[4], 128)],
                outputs: vec![view(&[], 256), view(&[9], 512)],
            },
        )
        .unwrap()
    }

    #[test]
    fn mixed_output_axes_infer_a_common_row_scope_with_two_steps() {
        let region = row_region();
        let plan = plan_schedule(
            &region,
            Some(&TileSpec {
                sizes: BTreeMap::from([("r".into(), 1), ("k".into(), 4)]),
            }),
        )
        .unwrap()
        .expect("mixed output tiling requires phases");
        assert_eq!(plan.tiles, BTreeMap::from([("r".into(), 1)]));
        assert_eq!(plan.steps.len(), 2);
    }

    #[test]
    fn equal_output_axes_keep_topological_assignments_in_one_group() {
        let mut region = row_region();
        region.program.assignments.swap(0, 2);
        region = validate_program(&region.program, &region.bindings).unwrap();
        let plan = plan_schedule(&region, Some(&row_tiling()))
            .unwrap()
            .unwrap();
        let ScopedStep::Group(indices) = &plan.steps[0] else {
            panic!("row statistics group");
        };
        assert_eq!(indices, &[2, 1]);
        let ScopedStep::Tiles(output) = &plan.steps[1] else {
            panic!("output loop");
        };
        assert_eq!(output.tiles, BTreeMap::from([("k".into(), 4)]));
        let ScopedStep::Group(indices) = &output.steps[0] else {
            panic!("output group");
        };
        assert_eq!(indices, &[0]);
    }

    #[test]
    fn only_cross_group_temporaries_receive_scope_ownership() {
        let plan = plan_schedule(&row_region(), Some(&row_tiling()))
            .unwrap()
            .unwrap();
        assert_eq!(plan.local_values, ["S"]);
        let ScopedStep::Tiles(output) = &plan.steps[1] else {
            panic!("output loop");
        };
        assert!(output.local_values.is_empty());
    }

    #[test]
    fn consumed_boundary_outputs_are_retained_across_groups() {
        let mut region = row_region();
        region.program.temporaries.remove("S");
        region.program.outputs.insert(0, "S".into());
        region.bindings.outputs.insert(
            0,
            TensorView::new_full(Tensor::new(&[3], &DataType::Fp32, 256).unwrap()),
        );
        region = validate_program(&region.program, &region.bindings).unwrap();
        let plan = plan_schedule(&region, Some(&row_tiling()))
            .unwrap()
            .unwrap();
        assert_eq!(plan.local_values, ["S"]);
    }

    #[test]
    fn unchunked_mixed_axes_need_no_scoped_schedule() {
        for tiling in [
            None,
            Some(TileSpec {
                sizes: BTreeMap::from([("k".into(), 9)]),
            }),
            Some(TileSpec {
                sizes: BTreeMap::from([("k".into(), 32)]),
            }),
        ] {
            assert!(
                plan_schedule(&row_region(), tiling.as_ref())
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn chunking_only_a_common_output_axis_needs_no_scoped_schedule() {
        let tiling = TileSpec {
            sizes: BTreeMap::from([("r".into(), 1)]),
        };
        assert!(
            plan_schedule(&row_region(), Some(&tiling))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn inferred_common_axes_default_to_their_full_extent() {
        let tiling = TileSpec {
            sizes: BTreeMap::from([("k".into(), 4)]),
        };
        let plan = plan_schedule(&row_region(), Some(&tiling))
            .unwrap()
            .unwrap();
        assert_eq!(plan.tiles, BTreeMap::from([("r".into(), 3)]));
    }

    #[test]
    fn tiling_validation_rejects_zero_tile_sizes() {
        let mut tiling = row_tiling();
        tiling.sizes.insert("k".into(), 0);
        let error = validate_tiling(&row_region(), Some(&tiling)).unwrap_err();
        assert!(error.0.contains("positive"), "{error}");
    }

    #[test]
    fn tiling_validation_rejects_unknown_tile_axes() {
        let mut tiling = row_tiling();
        tiling.sizes.insert("missing".into(), 4);
        let error = validate_tiling(&row_region(), Some(&tiling)).unwrap_err();
        assert!(error.0.contains("unknown tile axis"), "{error}");
    }

    #[test]
    fn flat_tile_iteration_rejects_a_phased_plan() {
        let error = plan_tiles(&row_region(), Some(&row_tiling())).unwrap_err();
        assert!(error.0.contains("phased schedule"), "{error}");
    }

    #[test]
    fn group_allocator_treats_external_local_producers_as_available() {
        let mut region = row_region();
        region.program.assignments = vec![region.program.assignments[1].clone()];
        region.program.temporaries.retain(|name, _| name == "S");
        region.assignment_order = vec![0];
        let tile = RegionTile::new(
            BTreeMap::from([("r".into(), 0..1)]),
            BTreeMap::new(),
            &region,
        )
        .unwrap();
        let allocation = allocate_temporaries(&region, &tile).unwrap();
        // One FP32 value occupies an eight-byte aligned SRAM slot.
        assert_eq!(allocation.arena_bytes, 8);
        assert_eq!(allocation.slots[0].values, ["S"]);
    }

    #[test]
    fn ready_statistics_are_hoisted_without_splitting_an_elementwise_chain() {
        let region = pipeline_with_statistic(
            r"
reduce:
  op: sum
  over: [j]
  value:
    read: { tensor: B, indices: [j] }
",
        );
        let tiling = TileSpec {
            sizes: BTreeMap::from([("i".into(), 4)]),
        };
        let plan = plan_schedule(&region, Some(&tiling)).unwrap().unwrap();
        let ScopedStep::Group(statistics) = &plan.steps[0] else {
            panic!("statistics group");
        };
        assert_eq!(statistics, &[1]);
        let ScopedStep::Tiles(pipeline) = &plan.steps[1] else {
            panic!("pipeline loop");
        };
        let ScopedStep::Group(chain) = &pipeline.steps[0] else {
            panic!("pipeline group");
        };
        assert_eq!(chain, &[0, 2]);
        assert!(plan.local_values.is_empty());
    }

    #[test]
    fn ready_current_group_consumers_precede_a_new_dependent_statistic() {
        let region = pipeline_with_statistic(
            r"
reduce:
  op: sum
  over: [i]
  value:
    read: { tensor: T, indices: [i] }
",
        );
        let tiling = TileSpec {
            sizes: BTreeMap::from([("i".into(), 4)]),
        };
        let plan = plan_schedule(&region, Some(&tiling)).unwrap().unwrap();
        let ScopedStep::Tiles(pipeline) = &plan.steps[0] else {
            panic!("producer pipeline must run first");
        };
        let ScopedStep::Group(chain) = &pipeline.steps[0] else {
            panic!("pipeline group");
        };
        assert_eq!(chain, &[0, 2]);
        let ScopedStep::Group(statistics) = &plan.steps[1] else {
            panic!("dependent statistic group");
        };
        assert_eq!(statistics, &[1]);
        assert_eq!(plan.local_values, ["T"]);
    }
}
