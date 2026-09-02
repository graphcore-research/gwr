// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A custom operator with caller-provided machine operation counts.

use std::rc::Rc;

use gwr_engine::types::{SimError, SimResult};
use serde::{Deserialize, Serialize};

use super::{Operator, TensorPartition, TensorView};
use crate::processing_element::{ComputeCapabilities, MachineOp, MachineOpCounts};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorCustom {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub machine_ops: MachineOpCounts,
}

impl Operator for OperatorCustom {
    fn validate(
        &self,
        _inputs: &[Option<TensorView>],
        _outputs: &[Option<TensorView>],
    ) -> SimResult {
        Ok(())
    }

    fn compute_delay_ticks(
        &self,
        compute_capabilities: &Rc<ComputeCapabilities>,
        _inputs: &[Option<TensorView>],
        _outputs: &[Option<TensorView>],
    ) -> Result<usize, SimError> {
        Ok(
            compute_capabilities.ticks_for_ops(self.machine_ops.adds, MachineOp::Add)?
                + compute_capabilities.ticks_for_ops(self.machine_ops.muls, MachineOp::Mul)?
                + compute_capabilities
                    .ticks_for_ops(self.machine_ops.compares, MachineOp::Compare)?,
        )
    }

    fn compute_machine_ops(
        &self,
        _inputs: &[Option<TensorView>],
        _outputs: &[Option<TensorView>],
    ) -> Result<MachineOpCounts, SimError> {
        Ok(self.machine_ops)
    }

    fn partition_views(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
        _num_partitions: usize,
    ) -> Result<Vec<TensorPartition>, SimError> {
        Ok(vec![TensorPartition {
            inputs: input_views.to_vec(),
            outputs: output_views.to_vec(),
        }])
    }
}
