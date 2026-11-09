// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_timetable::collective::{CollectiveKind, Participant, lower};
use gwr_timetable::timetable_file::TensorConfigSection;
#[test]
fn algorithms_expand_to_real_transfers_and_reduction_work() {
    for (kind, input, output, transfers) in [
        (CollectiveKind::AllReduce, 4, 4, 4),
        (CollectiveKind::ReduceScatter, 4, 2, 4),
        (CollectiveKind::AllGather, 4, 8, 4),
        (
            CollectiveKind::AllToAllV {
                counts: vec![vec![2, 2], vec![2, 2]],
            },
            4,
            4,
            4,
        ),
    ] {
        let file = lower("group", &kind, &ranks(input, output), 0x3000, 8).unwrap();
        let graph = file.into_graph().unwrap();
        assert_eq!(
            graph.nodes().iter().filter(|n| n.is_transfer()).count(),
            transfers
        );
        let completion = graph
            .nodes()
            .iter()
            .find(|n| n.id() == "group_complete")
            .unwrap();
        assert!(completion.predecessors().len() >= 4);
        if matches!(
            kind,
            CollectiveKind::AllReduce | CollectiveKind::ReduceScatter
        ) {
            assert!(
                graph
                    .nodes()
                    .iter()
                    .any(|n| n.program().is_some_and(|p| !p.assignments.is_empty()))
            );
        }
    }
}
#[test]
fn zero_count_pairs_still_participate_in_completion() {
    let file = lower(
        "group",
        &CollectiveKind::AllToAllV {
            counts: vec![vec![4, 0], vec![0, 4]],
        },
        &ranks(4, 4),
        0,
        8,
    )
    .unwrap();
    let graph = file.into_graph().unwrap();
    assert_eq!(graph.nodes().iter().filter(|n| n.is_transfer()).count(), 2);
    let completion = graph
        .nodes()
        .iter()
        .find(|n| n.id() == "group_complete")
        .unwrap();
    assert_eq!(completion.predecessors().len(), 4);
}
#[test]
fn all_reduce_runs_transfers_and_counts_rank_reduction_adds() {
    use std::rc::Rc;

    use gwr_engine::engine::Engine;
    use gwr_models::processing_element::dispatch::Dispatch;
    use gwr_platform::Platform;
    use gwr_timetable::Timetable;
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
    let file = lower("group", &CollectiveKind::AllReduce, &ranks(4, 4), 0x3000, 8).unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    assert_eq!(
        platform.pe("pe0").unwrap().machine_ops().by_operation["add"],
        4
    );
    // Two gather reads, scratch reduction reread, two broadcast reads.
    assert_eq!(platform.pe("pe0").unwrap().memory_traffic_bytes(), (96, 80));
}
#[test]
fn entirely_empty_rank_retains_protocol_completion_dependency() {
    let mut participants = ranks(4, 4);
    participants[1].input.shape = vec![0];
    participants[1].output.shape = vec![0];
    let file = lower(
        "empty",
        &CollectiveKind::AllToAllV {
            counts: vec![vec![4, 0], vec![0, 0]],
        },
        &participants,
        0,
        8,
    )
    .unwrap();
    let graph = file.into_graph().unwrap();
    assert_eq!(graph.nodes().iter().filter(|n| n.is_transfer()).count(), 1);
    let completion = graph
        .nodes()
        .iter()
        .find(|n| n.id() == "empty_complete")
        .unwrap();
    assert_eq!(completion.predecessors().len(), 3);
}
fn ranks(input: usize, output: usize) -> Vec<Participant> {
    (0..2)
        .map(|i| Participant {
            pe: "pe0".into(),
            input: TensorConfigSection {
                addr: 0x1000 + i * 0x100,
                dtype: DataType::Fp32,
                shape: vec![input],
            },
            output: TensorConfigSection {
                addr: 0x2000 + i * 0x100,
                dtype: DataType::Fp32,
                shape: vec![output],
            },
        })
        .collect()
}
