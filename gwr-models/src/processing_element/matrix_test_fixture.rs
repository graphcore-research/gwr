// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Shared PE and memory setup for native execution and private runner tests.
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::types::DeviceId;
use yaml_serde::from_str;

use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::MemoryMap;
use crate::memory::{Memory, MemoryConfig};
use crate::processing_element::task::ProgramTaskConfig;
use crate::processing_element::tensors::dtype::DataType;
use crate::processing_element::tensors::{Tensor, TensorView};
use crate::processing_element::{ProcessingElement, ProcessingElementConfig};

const KIBIBYTE: usize = 1024;
pub const LSU_ACCESS_BYTES: usize = 64 * KIBIBYTE;
pub const SRAM_BYTES: usize = LSU_ACCESS_BYTES;
pub const MEMORY_CAPACITY_BYTES: usize = 4 * LSU_ACCESS_BYTES;
pub const MEMORY_BYTES_PER_TICK: usize = LSU_ACCESS_BYTES;
pub const MEMORY_PACKET_SERVICE_TICKS: u64 =
    LSU_ACCESS_BYTES.div_ceil(MEMORY_BYTES_PER_TICK) as u64;
pub const MATRIX_OPERAND_COUNT: usize = 2;
pub const MATRIX_MACS_PER_TICK: usize = 256;
pub const MATRIX_SERVICE_M: usize = 16;
pub const MATRIX_SERVICE_N: usize = 16;
pub const MATRIX_SERVICE_K: usize = 16;
pub const BYTES_PER_BF16: usize = 2;
pub const BYTES_PER_FP32: usize = 4;
pub const MEMORY_BASE_ADDRESS: u64 = 0;
pub const LHS_BASE_ADDRESS: u64 = MEMORY_BASE_ADDRESS;
pub const RHS_BASE_ADDRESS: u64 = LHS_BASE_ADDRESS + LSU_ACCESS_BYTES as u64;
pub const OUTPUT_BASE_ADDRESS: u64 = RHS_BASE_ADDRESS + LSU_ACCESS_BYTES as u64;
const LSU_REQUEST_SLOTS: usize = 16;
const MEMORY_DEVICE_ID: DeviceId = DeviceId(0);
const PE_DEVICE_ID: DeviceId = DeviceId(1);

pub fn matrix_program_task(m: usize, n: usize, k: usize) -> ProgramTaskConfig {
    ProgramTaskConfig {
        id: "matrix".into(),
        tiling: None,
        program: from_str(
            r"
inputs: [A, B]
outputs: [Y]
assignments:
 - output: Y
   axes: [m, n]
   expr:
     matrix_mac:
       over: [k]
       accumulator: fp32
       args:
         - {read: {tensor: A, indices: [m, k]}}
         - {read: {tensor: B, indices: [k, n]}}
",
        )
        .unwrap(),
        inputs: vec![
            full_tensor_view(&[m, k], DataType::Bf16, LHS_BASE_ADDRESS),
            full_tensor_view(&[k, n], DataType::Bf16, RHS_BASE_ADDRESS),
        ],
        outputs: vec![full_tensor_view(
            &[m, n],
            DataType::Fp32,
            OUTPUT_BASE_ADDRESS,
        )],
    }
}

pub fn full_tensor_view(shape: &[usize], dtype: DataType, base: u64) -> TensorView {
    TensorView::new_full(Tensor::new(shape, &dtype, base).unwrap())
}

pub fn matrix_processing_element_config(
    ops_per_tick: usize,
    matrix_service_tiles_enabled: bool,
    accumulator_capacity: Option<u64>,
    replicated: bool,
) -> ProcessingElementConfig {
    let policy = if matrix_service_tiles_enabled {
        format!(
            ", matrix_service_tile: {{m: {MATRIX_SERVICE_M}, n: {MATRIX_SERVICE_N}, k: {MATRIX_SERVICE_K}}}"
        )
    } else {
        String::new()
    };
    let budget = accumulator_capacity
        .map(|capacity| format!("matrix_accumulator_capacity_bytes: {capacity}, "))
        .unwrap_or_default();
    let execution_unit_config = format!(
        "{budget}capabilities: [{{class: matrix, input_dtypes: [[bf16,bf16]], ops_per_tick: {ops_per_tick}, accumulator: fp32{policy}}}]"
    );
    let compute = if replicated {
        format!(
            "execution_units: {{matrix0: {{{execution_unit_config}}}, matrix1: {{{execution_unit_config}}}}}"
        )
    } else {
        format!("execution_units: {{matrix0: {{{execution_unit_config}}}}}")
    };
    ProcessingElementConfig {
        num_active_requests: LSU_REQUEST_SLOTS,
        lsu_access_bytes: LSU_ACCESS_BYTES,
        overhead_size_bytes: 0,
        sram_bytes: SRAM_BYTES,
        compute: from_str(&compute).unwrap(),
        operation_classes: from_str("matrix: {operations: [matrix_mac], arity: 2}").unwrap(),
    }
}

pub fn processing_element_and_memory_fixture(
    engine: &mut Engine,
    config: &ProcessingElementConfig,
    memory_name: &str,
) -> (Rc<ProcessingElement>, Rc<Memory<MemoryAccess>>) {
    let clock = engine.default_clock();
    let mut map = MemoryMap::new();
    map.insert(
        MEMORY_BASE_ADDRESS,
        MEMORY_CAPACITY_BYTES as u64,
        MEMORY_DEVICE_ID,
    )
    .unwrap();
    let map = Rc::new(map);
    let pe = ProcessingElement::new_and_register(
        engine,
        &clock,
        engine.top(),
        "pe",
        &map,
        config,
        PE_DEVICE_ID,
    )
    .unwrap();
    let memory = Memory::new_and_register(
        engine,
        &clock,
        engine.top(),
        memory_name,
        MemoryConfig::new(
            MEMORY_BASE_ADDRESS,
            MEMORY_CAPACITY_BYTES,
            MEMORY_BYTES_PER_TICK,
            0,
        ),
    )
    .unwrap();
    pe.connect_port_tx(memory.port_rx()).unwrap();
    memory.connect_port_tx(pe.port_rx()).unwrap();
    (pe, memory)
}
