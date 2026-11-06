// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_engine::types::SimError;

use super::access::{
    build_tile_output_views, collect_assignment_input_reads, collect_initial_input_reads,
};
use super::invocation::AssignmentInvocations;
use crate::processing_element::program::PreparedProgram;
use crate::processing_element::program::validate::RegionBindings;
use crate::processing_element::task::ProgramTaskConfig;

impl ProgramTaskConfig {
    /// Bind and estimate boundary traffic without issuing requests.
    pub fn planned_traffic_bytes(&self) -> Result<(usize, usize), SimError> {
        PreparedProgram::new(
            &self.program,
            RegionBindings {
                inputs: self.inputs.clone(),
                outputs: self.outputs.clone(),
            },
            self.tiling.as_ref(),
        )?
        .planned_traffic_bytes()
    }
}

impl PreparedProgram {
    /// Hardware-independent boundary traffic from the canonical lazy schedule.
    /// Analysis uses bounded memory without collecting every invocation;
    /// traversal time can still scale with invocation count.
    pub fn planned_traffic_bytes(&self) -> Result<(usize, usize), SimError> {
        let mut read_bytes = 0usize;
        let mut written_bytes = 0usize;
        self.visit_groups(&mut |plan, tile, _| {
            let mut add_read = |read: &super::access::InputRead<'_>, chunk: &BTreeMap<_, _>| {
                read_bytes = read_bytes
                    .checked_add(read.plan_access(tile, plan.region(), chunk)?.bytes())
                    .ok_or_else(|| SimError("planned read traffic overflows".into()))?;
                Ok::<_, SimError>(())
            };
            for read in collect_initial_input_reads(plan) {
                add_read(&read, &BTreeMap::new())?;
            }
            for &index in plan.assignments {
                let reads = collect_assignment_input_reads(plan, index);
                for invocation in AssignmentInvocations::new(plan.region(), index, tile)? {
                    let invocation = invocation?;
                    if invocation.read_label.is_some() {
                        for read in &reads {
                            add_read(read, &invocation.chunk)?;
                        }
                    }
                }
            }
            for view in build_tile_output_views(plan, tile)? {
                written_bytes = written_bytes
                    .checked_add(view.layout().num_access_bytes())
                    .ok_or_else(|| SimError("planned write traffic overflows".into()))?;
            }
            Ok(())
        })?;
        Ok((read_bytes, written_bytes))
    }
}
