// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::types::DeviceId;
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::processing_element::compute::{
    CapabilityConfig, CapabilityKind, ComputeConfig, ExecutionUnitConfig, OperationClass,
};
use gwr_models::processing_element::program::syntax::{
    AffineSpec, AssignmentSpec, BoundsMode, ExprSpec, IndexSpec, ProgramSpec, ReductionOp,
};
use gwr_models::processing_element::program::{PreparedProgram, ProgramBindings};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use gwr_models::processing_element::{ProcessingElement, ProcessingElementConfig};

const REDUCTION_ELEMENTS: usize = 16;
const INPUT_ELEMENT_OFFSET: usize = 1;
const INPUT_ELEMENTS: usize = REDUCTION_ELEMENTS + INPUT_ELEMENT_OFFSET;
const OUTPUT_ADDRESS: u64 = 1024;
const MEMORY_CAPACITY_BYTES: u64 = 4096;
const SRAM_BYTES: usize = 16;

fn offset_reduction_programs() -> impl Iterator<Item = (&'static str, PreparedProgram)> {
    [
        ("direct", IndexSpec::Axis("k".into())),
        (
            "identity_affine",
            IndexSpec::Affine {
                affine: AffineSpec {
                    terms: BTreeMap::from([("k".into(), 1)]),
                    offset: 0,
                },
            },
        ),
    ]
    .into_iter()
    .map(|(label, index)| {
        let read = ExprSpec::Read {
            tensor: "A".into(),
            indices: vec![index],
            bounds: BoundsMode::RequireInRange,
        };
        let spec = ProgramSpec {
            inputs: vec!["A".into()],
            outputs: vec!["Y".into()],
            temporaries: BTreeMap::new(),
            extents: BTreeMap::from([("k".into(), REDUCTION_ELEMENTS)]),
            assignments: vec![AssignmentSpec {
                output: Some("Y".into()),
                indices_output: None,
                axes: vec![],
                expr: ExprSpec::Reduce {
                    op: ReductionOp::Sum,
                    over: vec!["k".into()],
                    value: Box::new(read),
                },
            }],
        };
        let program = PreparedProgram::new(
            &spec,
            ProgramBindings {
                inputs: vec![
                    TensorView::new(
                        Tensor::new(&[INPUT_ELEMENTS], &DataType::Int4, 0).unwrap(),
                        &[REDUCTION_ELEMENTS],
                        &[INPUT_ELEMENT_OFFSET],
                    )
                    .unwrap(),
                ],
                outputs: vec![TensorView::new_full(
                    Tensor::new(&[], &DataType::Int4, OUTPUT_ADDRESS).unwrap(),
                )],
            },
            None,
        )
        .unwrap();
        (label, program)
    })
}

fn processing_element_fixture(engine: &mut Engine) -> Rc<ProcessingElement> {
    let clock = engine.default_clock();
    let mut memory_map = MemoryMap::new();
    memory_map
        .insert(0, MEMORY_CAPACITY_BYTES, DeviceId(1))
        .unwrap();
    let config = ProcessingElementConfig {
        num_active_requests: 1,
        lsu_access_bytes: 1,
        overhead_size_bytes: 0,
        sram_bytes: SRAM_BYTES,
        compute: ComputeConfig {
            execution_units: BTreeMap::from([(
                "vector".into(),
                ExecutionUnitConfig {
                    matrix_accumulator_capacity_bytes: None,
                    capabilities: vec![CapabilityConfig {
                        class: "add".into(),
                        input_dtypes: None,
                        ops_per_tick: 1.0,
                        kind: CapabilityKind::Scalar,
                    }],
                },
            )]),
        },
        operation_classes: BTreeMap::from([(
            "add".into(),
            OperationClass {
                operations: vec!["add".into()],
                arity: 2,
                flops_per_op: 1,
            },
        )]),
    };
    ProcessingElement::new_and_register(
        engine,
        &clock,
        engine.top(),
        "pe",
        &Rc::new(memory_map),
        &config,
        DeviceId(2),
    )
    .unwrap()
}

#[test]
fn packed_offset_reduction_uses_same_sram_for_direct_and_identity_affine_reads() {
    for (label, program) in offset_reduction_programs() {
        // Sixteen nibbles at element offset one touch nine physical bytes,
        // but occupy eight packed bytes plus one eight-byte accumulator slot.
        assert_eq!(program.planned_traffic_bytes().unwrap(), (9, 1), "{label}");
        assert_eq!(program.peak_sram_bytes().unwrap(), SRAM_BYTES, "{label}");
    }
}

#[test]
fn packed_offset_reduction_prepares_with_16_bytes_for_direct_and_identity_affine_reads() {
    for (label, program) in offset_reduction_programs() {
        let mut engine = Engine::default();
        let pe = processing_element_fixture(&mut engine);
        assert_eq!(program.planned_traffic_bytes().unwrap(), (9, 1), "{label}");
        let prepared = pe
            .prepare_program(label.into(), Rc::new(program))
            .expect(label);
        assert_eq!(prepared.peak_sram_bytes(), SRAM_BYTES, "{label}");
    }
}
