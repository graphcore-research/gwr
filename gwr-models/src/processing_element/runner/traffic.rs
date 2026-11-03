// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::ops::Range;

use gwr_engine::types::SimError;

use super::access::{
    InputRead, build_tile_output_views, collect_assignment_input_reads, collect_initial_input_reads,
};
use super::schedule::{collect_retained_values, visit_groups};
use super::tile::build_group_config;
use crate::processing_element::program::schedule::plan_schedule;
use crate::processing_element::program::syntax::ExprSpec;
use crate::processing_element::program::tile::{RegionTile, plan_tiles};
use crate::processing_element::program::validate::{
    RegionBindings, ValidatedRegion, validate_program,
};
use crate::processing_element::task::ProgramTaskConfig;

impl ProgramTaskConfig {
    /// Boundary traffic implied by dependency-derived phases and tile groups.
    pub fn planned_traffic_bytes(&self) -> Result<(usize, usize), SimError> {
        let region = validate_program(
            &self.program,
            &RegionBindings {
                inputs: self.inputs.clone(),
                outputs: self.outputs.clone(),
            },
        )?;
        let mut totals = (0usize, 0usize);
        if let Some(schedule) = plan_schedule(&region, self.tiling.as_ref())? {
            let retained = collect_retained_values(&schedule);
            visit_groups(
                &schedule,
                &region,
                self.tiling.as_ref(),
                &mut |indices, tile, _local| {
                    let (phase, phase_region) =
                        build_group_config(self, &region, indices, &retained);
                    add_traffic(
                        &mut totals,
                        calculate_tile_traffic_bytes(&phase, &phase_region, tile)?,
                    )
                },
            )?;
        } else {
            for tile in plan_tiles(&region, self.tiling.as_ref())?.tiles() {
                add_traffic(
                    &mut totals,
                    calculate_tile_traffic_bytes(self, &region, &tile)?,
                )?;
            }
        }
        Ok(totals)
    }
}

fn calculate_tile_traffic_bytes(
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    tile: &RegionTile,
) -> Result<(usize, usize), SimError> {
    let mut read_bytes = 0usize;
    add_input_read_bytes(
        &mut read_bytes,
        &collect_initial_input_reads(config),
        &BTreeMap::new(),
        tile,
        region,
    )?;
    for (index, assignment) in config.program.assignments.iter().enumerate() {
        let reads = collect_assignment_input_reads(config, index);
        if reads.is_empty() {
            continue;
        }
        let axes = match &assignment.expr {
            ExprSpec::Reduce { over, .. } | ExprSpec::MatrixMac { over, .. } => over.as_slice(),
            _ => &[],
        };
        for chunk in tile.reduction_chunks_for(axes) {
            add_input_read_bytes(&mut read_bytes, &reads, &chunk, tile, region)?;
        }
    }
    let written_bytes = tile_output_bytes(config, tile)?;
    Ok((read_bytes, written_bytes))
}

fn tile_output_bytes(config: &ProgramTaskConfig, tile: &RegionTile) -> Result<usize, SimError> {
    let mut written_bytes = 0usize;
    for view in build_tile_output_views(config, tile)? {
        written_bytes = written_bytes
            .checked_add(view.layout().num_access_bytes())
            .ok_or_else(|| SimError("planned write traffic overflows".into()))?;
    }
    Ok(written_bytes)
}

fn add_traffic(totals: &mut (usize, usize), traffic: (usize, usize)) -> Result<(), SimError> {
    totals.0 = totals
        .0
        .checked_add(traffic.0)
        .ok_or_else(|| SimError("planned read traffic overflows".into()))?;
    totals.1 = totals
        .1
        .checked_add(traffic.1)
        .ok_or_else(|| SimError("planned write traffic overflows".into()))?;
    Ok(())
}

fn add_input_read_bytes(
    total: &mut usize,
    reads: &[InputRead<'_>],
    chunk: &BTreeMap<String, Range<usize>>,
    tile: &RegionTile,
    region: &ValidatedRegion,
) -> Result<(), SimError> {
    for read in reads {
        *total = total
            .checked_add(read.plan_access(tile, region, chunk)?.bytes())
            .ok_or_else(|| SimError("planned read traffic overflows".into()))?;
    }
    Ok(())
}
