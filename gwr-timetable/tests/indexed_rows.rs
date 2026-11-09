// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::collections::BTreeMap;

use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_timetable::indexed::{RowSelection, lower_gather, lower_scatter};
use gwr_timetable::timetable_file::{
    NodeSection, ProgramDefinition, ProgramSection, TensorConfigSection,
};
#[test]
fn selected_rows_change_exact_addresses_and_coalesce_contiguous_runs() {
    for (rows, expected) in [
        (
            vec![0, 1],
            std::iter::once(0x1000..0x1010).collect::<Vec<_>>(),
        ),
        (
            vec![1, 1, 3],
            vec![0x1008..0x1010, 0x1008..0x1010, 0x1018..0x1020],
        ),
    ] {
        let mut file = lower_gather(&selection(rows)).unwrap();
        file.nodes.push(router());
        let graph = file.into_graph().unwrap();
        let ranges = graph
            .nodes()
            .iter()
            .enumerate()
            .filter(|(_, node)| node.is_transfer())
            .flat_map(|(idx, _)| {
                graph.compute_views(idx).unwrap().inputs()[0]
                    .as_ref()
                    .unwrap()
                    .address_ranges()
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(ranges, expected);
        let router_idx = graph
            .nodes()
            .iter()
            .position(|node| node.id() == "router")
            .unwrap();
        for node in graph
            .nodes()
            .iter()
            .filter(|node| node.is_transfer() || node.id() == "selected_complete")
        {
            assert!(node.predecessors().contains(&router_idx));
        }
    }
}
#[test]
fn empty_selection_keeps_producer_dependency_without_payload() {
    let mut file = lower_gather(&selection(vec![])).unwrap();
    file.nodes.push(router());
    let graph = file.into_graph().unwrap();
    assert_eq!(
        graph
            .nodes()
            .iter()
            .filter(|node| node.is_transfer())
            .count(),
        0
    );
    let complete = graph
        .nodes()
        .iter()
        .find(|node| node.id() == "selected_complete")
        .unwrap();
    assert_eq!(complete.predecessors().len(), 1);
}
#[test]
fn bounds_and_duplicate_scatter_policy_are_explicit() {
    assert!(lower_gather(&selection(vec![4])).is_err());
    let mut scatter = selection(vec![1, 1]);
    std::mem::swap(&mut scatter.source, &mut scatter.destination);
    assert!(lower_scatter(&scatter).is_err());
    scatter.rows = vec![1, 3];
    let mut file = lower_scatter(&scatter).unwrap();
    file.nodes.push(router());
    file.validate().unwrap();
}
#[test]
fn selected_traffic_counts_real_transfer_requests() {
    use std::rc::Rc;

    use gwr_engine::engine::Engine;
    use gwr_models::processing_element::dispatch::Dispatch;
    use gwr_platform::Platform;
    use gwr_timetable::Timetable;
    for (rows, bytes) in [(vec![0, 1], 16), (vec![1, 1, 3], 24), (vec![], 0)] {
        let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
            1000,
            gwr_track::log::Level::Trace,
        ));
        let tracking: gwr_track::Tracker = tracker;
        let mut engine = Engine::new(&tracking);
        let clock = engine.default_clock();
        let platform = Rc::new(
            Platform::from_string(
                &engine,
                &clock,
                include_str!("../examples/compute_region_platform.yaml"),
            )
            .unwrap(),
        );
        let mut file = lower_gather(&selection(rows)).unwrap();
        file.nodes.push(router());
        let timetable =
            Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
        let dispatcher: Rc<dyn Dispatch> = timetable.clone();
        platform.attach_dispatcher(&dispatcher);
        engine.run().unwrap();
        timetable.check_tasks_complete().unwrap();
        assert_eq!(
            platform.pe("pe0").unwrap().memory_traffic_bytes(),
            (bytes, bytes)
        );
    }
}
fn selection(rows: Vec<usize>) -> RowSelection {
    RowSelection {
        id: "selected".into(),
        pe: "pe0".into(),
        source: TensorConfigSection {
            addr: 0x1000,
            dtype: DataType::Fp32,
            shape: vec![4, 2],
        },
        destination: TensorConfigSection {
            addr: 0x2000,
            dtype: DataType::Fp32,
            shape: vec![rows.len().max(1), 2],
        },
        rows,
        selection_producer: "router".into(),
        buffer_bytes: 16,
    }
}
fn router() -> NodeSection {
    NodeSection::Compute {
        pipeline: None,
        id: "router".into(),
        pe: Some("pe0".into()),
        program: ProgramSection::Inline(Box::new(ProgramDefinition {
            inputs: vec![],
            metadata_inputs: vec![],
            indexed_metadata_inputs: vec![],
            local_round_policy: None,
            outputs: vec![],
            assignments: vec![],
            temporaries: BTreeMap::new(),
            extents: BTreeMap::new(),
            calls: vec![],
        })),
        tiling: None,
        input_views: None,
        output_views: None,
    }
}
