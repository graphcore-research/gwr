// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! SRAM slot sizing and binding of persistent reduction state.

use gwr_engine::types::SimError;

use crate::processing_element::tensors::BITS_PER_BYTE;
use crate::processing_element::tensors::dtype::DataType;

/// Byte granularity used for program SRAM slot reservations.
pub const SRAM_SLOT_ALIGNMENT_BYTES: usize = 8;

/// Round a slot's reserved size up to the shared SRAM alignment.
pub(crate) fn align_sram_slot_bytes(bytes: usize) -> Result<usize, SimError> {
    bytes
        .checked_next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES)
        .ok_or_else(|| SimError("SRAM slot alignment overflows".into()))
}

pub(in crate::processing_element) fn select_reduction_state_binding(
    inputs: &[DataType],
    output: DataType,
    output_elements: usize,
    persistent: bool,
) -> Result<ReductionStateBinding, SimError> {
    select_reduction_state_binding_for_optional_output(
        inputs,
        Some(output),
        output_elements,
        persistent,
    )
}

pub(in crate::processing_element) fn select_reduction_state_binding_for_optional_output(
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
        .div_ceil(BITS_PER_BYTE);
    Ok(ReductionStateBinding::Scratch { bytes })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::processing_element) enum ReductionStateBinding {
    EngineTransient,
    Destination,
    Scratch { bytes: usize },
}
