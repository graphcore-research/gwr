// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A custom operator with caller-provided machine operation counts.

use std::rc::Rc;

use gwr_engine::types::{SimError, SimResult};
use serde::{Deserialize, Serialize};

use super::{Operator, TensorPartition, TensorPartitions, TensorView};
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
        let add_ticks =
            compute_capabilities.ticks_for_ops(self.machine_ops.adds, MachineOp::Add)?;
        let mul_ticks =
            compute_capabilities.ticks_for_ops(self.machine_ops.muls, MachineOp::Mul)?;
        let compare_ticks =
            compute_capabilities.ticks_for_ops(self.machine_ops.compares, MachineOp::Compare)?;
        add_ticks
            .checked_add(mul_ticks)
            .and_then(|ticks| ticks.checked_add(compare_ticks))
            .ok_or_else(|| SimError("Custom operator compute delay overflows".to_string()))
    }

    fn compute_machine_ops(
        &self,
        _inputs: &[Option<TensorView>],
        _outputs: &[Option<TensorView>],
    ) -> Result<MachineOpCounts, SimError> {
        Ok(self.machine_ops)
    }

    fn max_partition_count(
        &self,
        _inputs: &[Option<TensorView>],
        _outputs: &[Option<TensorView>],
    ) -> Result<usize, SimError> {
        Ok(1)
    }

    fn partition_views<'a>(
        &'a self,
        inputs: &'a [Option<TensorView>],
        outputs: &'a [Option<TensorView>],
        _num_partitions: usize,
    ) -> Result<TensorPartitions<'a>, SimError> {
        Ok(Box::new(std::iter::once(Ok(TensorPartition {
            inputs: inputs.to_vec(),
            outputs: outputs.to_vec(),
        }))))
    }
}
