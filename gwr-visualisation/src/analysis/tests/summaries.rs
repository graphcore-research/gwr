// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::Path;

use gwr_models::processing_element::MachineOpCounts;
use gwr_models::processing_element::operators::OperatorCustom;
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::task::ComputeOp;
use gwr_platform::types::PlatformConfig;

use super::{build_report, compute, data, graph, report, small_graph, tensor};

#[test]
fn summarizes_compute_and_tensor_work_by_layer_and_pe() {
    let data = report(&small_graph());

    assert_eq!(data.summary.compute_nodes, 3);
    assert_eq!(data.summary.total_machine_ops, 22_579_200);
    assert_eq!(data.summary.tensor_nodes, 6);
    assert_eq!(data.summary.data_edges, 9);
    assert_eq!(data.summary.active_pes, 3);
    assert_eq!(data.summary.total_tensor_read_bytes, 1_204_224);
    assert_eq!(data.summary.total_tensor_write_bytes, 802_816);
    assert_eq!(data.layers.len(), 2);
    assert_eq!(data.layers[0].name, "layer 1");
    assert_eq!(data.layers[0].compute_nodes, 2);
    assert_eq!(data.layers[0].machine_ops.total, 150_528);
    assert_eq!(data.layers[0].tensor_read_bytes, 802_816);
    assert_eq!(data.layers[0].tensor_write_bytes, 602_112);

    let pe = data.pes.iter().find(|pe| pe.name == "pe_0_0").unwrap();
    assert_eq!(pe.total_nodes, 1);
    assert_eq!(pe.machine_ops.adds, 100_352);
    assert_eq!(pe.tensor_read_bytes, 401_408);
    assert_eq!(pe.tensor_write_bytes, 401_408);
    assert!(data.ops.contains(&"add".to_string()));
    assert!(data.ops.contains(&"gemm".to_string()));
}

#[test]
fn counts_tensorless_custom_operations() {
    let graph = graph(
        vec![compute(
            "custom",
            ComputeOp::Custom(OperatorCustom {
                name: Some("activation".to_string()),
                machine_ops: MachineOpCounts {
                    adds: 3,
                    muls: 5,
                    compares: 7,
                },
            }),
            Some("pe0"),
            0,
            0,
        )],
        vec![],
    );

    let data = report(&graph);
    assert_eq!(data.summary.total_machine_ops, 15);
    assert_eq!(data.pes[0].machine_ops.adds, 3);
    assert_eq!(data.pes[0].machine_ops.muls, 5);
    assert_eq!(data.pes[0].machine_ops.compares, 7);
    assert_eq!(data.ops, ["activation"]);
}

#[test]
fn rejects_report_machine_operation_totals_that_overflow() {
    let operation = || {
        ComputeOp::Custom(OperatorCustom {
            name: None,
            machine_ops: MachineOpCounts {
                adds: usize::MAX,
                ..MachineOpCounts::default()
            },
        })
    };
    let graph = graph(
        vec![
            compute("first", operation(), Some("pe0"), 0, 0),
            compute("second", operation(), Some("pe0"), 0, 0),
        ],
        vec![],
    );

    let error = build_report(&graph, Path::new("overflow.yaml"), None, None).unwrap_err();
    assert!(error.to_string().contains("machine add count overflows"));
}

#[test]
fn reports_operator_count_overflow_with_the_compute_node() {
    let extent = 4_194_304;
    let bytes = (extent as u64 * extent as u64) / 2;
    let graph = graph(
        vec![
            tensor("a", 0, DataType::Int4, &[extent, extent]),
            tensor("b", bytes, DataType::Int4, &[extent, extent]),
            tensor("out", bytes * 2, DataType::Int4, &[extent, extent]),
            compute("gemm", ComputeOp::Gemm, Some("pe0"), 2, 1),
        ],
        vec![
            data("a", "gemm.0"),
            data("b", "gemm.1"),
            data("gemm", "out"),
        ],
    );

    let error = build_report(&graph, Path::new("gemm.yaml"), None, None).unwrap_err();
    assert!(error.to_string().contains("compute node 'gemm'"));
    assert!(error.to_string().contains("overflows"));
}

#[test]
fn joins_platform_coordinates_and_effective_pe_configuration() {
    let graph = graph(
        vec![
            compute("compute", ComputeOp::Add, Some("worker"), 2, 1),
            tensor("left", 0, DataType::Int8, &[1]),
            tensor("right", 1, DataType::Int8, &[1]),
            tensor("out", 2, DataType::Int8, &[1]),
        ],
        vec![
            data("left", "compute.0"),
            data("right", "compute.1"),
            data("compute", "out"),
        ],
    );
    let platform: PlatformConfig = serde_yaml::from_str(
        r"
memory_maps:
  - name: default
    devices: []
fabrics:
  - name: fabric0
    kind: functional
    columns: 12
    rows: 24
    config: {}
processing_elements:
  - name: worker
    memory_map: default
    config: {}
connections:
  - connect: [pe.worker, 'fabric.fabric0@(5,7)']
",
    )
    .unwrap();
    platform.validate().unwrap();

    let data = build_report(
        &graph,
        Path::new("timetable.yaml"),
        Some((&platform, Path::new("platform.yaml"))),
        None,
    )
    .unwrap();
    let pe = data.pes.iter().find(|pe| pe.name == "worker").unwrap();
    assert_eq!((pe.col, pe.row), (5, 7));
    assert_eq!(
        pe.platform_config.as_ref().unwrap().num_active_requests,
        Some(8)
    );
    assert_eq!(data.platform.as_ref().unwrap().cols, 12);
    assert_eq!(data.platform.as_ref().unwrap().rows, 24);
}
