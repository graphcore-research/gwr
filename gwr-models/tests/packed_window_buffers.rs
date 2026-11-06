// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::types::DeviceId;
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::processing_element::compute::{
    CapabilityConfig, CapabilityKind, ComputeConfig, ExecutionUnitConfig, OperationClass,
};
use gwr_models::processing_element::program::PreparedProgram;
use gwr_models::processing_element::program::syntax::{
    AffineSpec, AssignmentSpec, BoundsMode, ExprSpec, IndexSpec, ProgramSpec, ReductionOp,
};
use gwr_models::processing_element::program::validate::RegionBindings;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use gwr_models::processing_element::{ProcessingElement, ProcessingElementConfig};

fn check_offset_reduction(check_preparation: bool) {
    for (label, index) in [
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
    ] {
        let read = ExprSpec::Read {
            tensor: "A".into(),
            indices: vec![index],
            bounds: BoundsMode::RequireInRange,
        };
        let spec = ProgramSpec {
            inputs: vec!["A".into()],
            outputs: vec!["Y".into()],
            temporaries: BTreeMap::new(),
            extents: BTreeMap::from([("k".into(), 16)]),
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
            RegionBindings {
                inputs: vec![
                    TensorView::new(Tensor::new(&[17], &DataType::Int4, 0).unwrap(), &[16], &[1])
                        .unwrap(),
                ],
                outputs: vec![TensorView::new_full(
                    Tensor::new(&[], &DataType::Int4, 1024).unwrap(),
                )],
            },
            None,
        )
        .unwrap();
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let mut mm = MemoryMap::new();
        mm.insert(0, 4096, DeviceId(1)).unwrap();
        let config = ProcessingElementConfig {
            num_active_requests: 1,
            lsu_access_bytes: 1,
            overhead_size_bytes: 0,
            sram_bytes: 16,
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
        let pe = ProcessingElement::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "pe",
            &Rc::new(mm),
            &config,
            DeviceId(2),
        )
        .unwrap();
        // Sixteen nibbles at element offset one touch nine physical bytes,
        // but occupy eight packed bytes plus one eight-byte accumulator slot.
        assert_eq!(program.planned_traffic_bytes().unwrap(), (9, 1), "{label}");
        if check_preparation {
            let prepared = pe
                .prepare_program(label.into(), Rc::new(program))
                .expect(label);
            assert_eq!(prepared.peak_sram_bytes(), 16, "{label}");
        } else {
            assert_eq!(program.peak_sram_bytes().unwrap(), 16, "{label}");
        }
    }
}

#[test]
fn packed_offset_reduction_uses_same_sram_for_direct_and_identity_affine_reads() {
    check_offset_reduction(false);
}

#[test]
fn packed_offset_reduction_prepares_with_16_bytes_for_direct_and_identity_affine_reads() {
    check_offset_reduction(true);
}
