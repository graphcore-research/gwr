// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::path::Path;

use gwr_models::processing_element::MachineOpCounts;
use gwr_models::processing_element::operators::OperatorCustom;
use gwr_models::processing_element::task::ComputeOp;

use super::{build_report, compute, graph};
use crate::analysis::OverlayInput;
use crate::model::OverlayMetricMetadata;

fn custom() -> ComputeOp {
    ComputeOp::Custom(OperatorCustom {
        name: None,
        machine_ops: MachineOpCounts::default(),
    })
}

#[test]
fn adds_metadata_for_observed_overlay_values() {
    let graph = graph(
        vec![compute("compute", custom(), Some("pe0"), 0, 0)],
        vec![],
    );
    let overlay = OverlayInput {
        metrics: BTreeMap::from([(
            "declared".to_string(),
            OverlayMetricMetadata {
                label: Some("Declared".to_string()),
                unit: None,
            },
        )]),
        metrics_by_pe: BTreeMap::from([(
            "pe0".to_string(),
            BTreeMap::from([("declared".to_string(), 1.0), ("observed".to_string(), 2.0)]),
        )]),
    };

    let data = build_report(
        &graph,
        Path::new("timetable.yaml"),
        None,
        Some((&overlay, Path::new("overlay.json"))),
    )
    .unwrap();
    assert_eq!(
        data.overlay_metrics["declared"].label.as_deref(),
        Some("Declared")
    );
    assert!(data.overlay_metrics.contains_key("observed"));
    assert_eq!(data.pes[0].overlays["observed"], 2.0);
}

#[test]
fn warns_only_when_overlay_values_name_an_unknown_pe() {
    let graph = graph(
        vec![compute("compute", custom(), Some("pe0"), 0, 0)],
        vec![],
    );
    let overlay = OverlayInput {
        metrics: BTreeMap::new(),
        metrics_by_pe: BTreeMap::from([(
            "missing".to_string(),
            BTreeMap::from([("temperature".to_string(), 42.0)]),
        )]),
    };

    let data = build_report(
        &graph,
        Path::new("timetable.yaml"),
        None,
        Some((&overlay, Path::new("overlay.json"))),
    )
    .unwrap();
    assert_eq!(data.warnings, ["Overlay references unknown PE 'missing'"]);
    assert!(data.overlay_metrics.contains_key("temperature"));
}

#[test]
fn keeps_unassigned_compute_nodes_separate_from_named_pes() {
    let graph = graph(
        vec![
            compute("named", custom(), Some("unassigned"), 0, 0),
            compute("anonymous", custom(), None, 0, 0),
        ],
        vec![],
    );
    let overlay = OverlayInput {
        metrics: BTreeMap::new(),
        metrics_by_pe: BTreeMap::from([
            ("unassigned_1".to_string(), BTreeMap::new()),
            ("unassigned_2".to_string(), BTreeMap::new()),
        ]),
    };

    let data = build_report(
        &graph,
        Path::new("timetable.yaml"),
        None,
        Some((&overlay, Path::new("overlay.json"))),
    )
    .unwrap();
    assert_eq!(
        data.pes
            .iter()
            .map(|pe| (pe.name.as_str(), pe.total_nodes))
            .collect::<Vec<_>>(),
        [("unassigned", 1), ("unassigned_3", 1)]
    );
}
