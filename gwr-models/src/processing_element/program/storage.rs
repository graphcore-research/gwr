// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Physical binding of reduction state that survives an engine batch.

use gwr_engine::types::SimError;

use crate::processing_element::tensors::dtype::DataType;

pub fn reduction_state_binding_for_operands(
    inputs: &[DataType],
    output: DataType,
    output_elements: usize,
    persistent: bool,
) -> Result<ReductionStateBinding, SimError> {
    reduction_state_binding_for_optional_output(inputs, Some(output), output_elements, persistent)
}

pub fn reduction_state_binding_for_optional_output(
    inputs: &[DataType],
    output: Option<DataType>,
    output_elements: usize,
    persistent: bool,
) -> Result<ReductionStateBinding, SimError> {
    if !persistent {
        return Ok(ReductionStateBinding::EngineTransient);
    }
    let bits = inputs
        .iter()
        .map(DataType::num_bits)
        .max()
        .unwrap_or(0)
        .max(output.map_or(0, |dtype| dtype.num_bits()));
    if output.is_some_and(|dtype| dtype.num_bits() >= bits) {
        return Ok(ReductionStateBinding::Destination);
    }
    let bytes = output_elements
        .checked_mul(bits)
        .ok_or_else(|| SimError("reduction scratch size overflows".into()))?
        .div_ceil(8);
    Ok(ReductionStateBinding::Scratch { bytes })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReductionStateBinding {
    EngineTransient,
    Destination,
    Scratch { bytes: usize },
}
