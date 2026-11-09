// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_models::memory::atomic::*;
use gwr_timetable::atomic::*;
use gwr_timetable::timetable_file::TimetableFile;
fn cfg() -> AtomicServiceConfig {
    AtomicServiceConfig {
        capacity: 4,
        bytes_per_tick: 8,
        delay_ticks: 0,
        bf16_pair_per_tick: Some(1),
        i32_fetch_add_per_tick: Some(1),
        lane_interleave_bytes: 4,
        lane_bytes_per_tick: vec![8],
    }
}
fn pair(node: &str) -> EffectClaim {
    EffectClaim {
        node: node.into(),
        device: 7,
        address: 4096,
        effect: Effect::Atomic {
            participant: if node == "a" { 0 } else { 1 },
            word: AtomicWord {
                device: 7,
                address: 4096,
                invocation: 9,
                generation: 1,
                operation: AtomicOperation::Bf16PairFadd,
                scope: AtomicScope::Agent,
                ordering: AtomicOrdering::Monotonic,
            },
        },
    }
}
fn graph(reader_address: u64, ordered: bool) -> gwr_timetable::TimetableGraph {
    let dependency = if ordered {
        "- {from: a, to: reader, kind: dependency}\n- {from: b, to: reader, kind: dependency}"
    } else {
        ""
    };
    let source = format!(
        "nodes:\n- {{id: backing, kind: tensor, config: {{addr: 4096, dtype: bf16, shape: [2]}}}}\n- {{id: input, kind: tensor, config: {{addr: {reader_address}, dtype: int8, shape: [4]}}}}\n- {{id: a, kind: compute, program: {{inputs: [], outputs: [], assignments: []}}}}\n- {{id: b, kind: compute, program: {{inputs: [], outputs: [], assignments: []}}}}\n- {{id: reader, kind: compute, program: {{inputs: [A], outputs: [], assignments: []}}}}\nedges:\n- {{from: input, to: reader, kind: data}}\n{dependency}\n"
    );
    TimetableFile::from_string(&source)
        .unwrap()
        .into_graph()
        .unwrap()
}
fn memories() -> Vec<MemoryRegion> {
    vec![MemoryRegion {
        device: 7,
        base: 4096,
        bytes: 256,
        path: AtomicPath::CacheFreeDirect,
        service: cfg(),
    }]
}
fn bindings() -> Vec<WordBinding> {
    vec![WordBinding {
        device: 7,
        address: 4096,
        tensor: "backing".into(),
    }]
}
#[test]
fn exact_pair_overlap_is_legal_but_halfword_foreign_generation_or_unordered_ordinary_is_not() {
    let g = graph(8192, false);
    let claims = vec![pair("a"), pair("b")];
    admit(&g, &memories(), &bindings(), &claims, &[]).unwrap();
    let mut foreign = claims.clone();
    if let Effect::Atomic { word, .. } = &mut foreign[1].effect {
        word.generation = 2;
    }
    assert!(
        admit(&g, &memories(), &bindings(), &foreign, &[])
            .unwrap_err()
            .to_string()
            .contains("incompatible unordered")
    );
    let mut half = claims.clone();
    half[1].address += 2;
    assert!(admit(&g, &memories(), &bindings(), &half, &[]).is_err());
    assert!(
        admit(&graph(4098, false), &memories(), &bindings(), &claims, &[])
            .unwrap_err()
            .to_string()
            .contains("ordinary/typed")
    );
    admit(&graph(4096, true), &memories(), &bindings(), &claims, &[]).unwrap();
    let mut cache = memories();
    cache[0].path = AtomicPath::Cache;
    assert!(admit(&g, &cache, &bindings(), &claims, &[]).is_err());
}
#[test]
fn unchanged_ordinary_graph_still_rejects_unordered_write_overlap_and_same_task_alias() {
    let source = "nodes: [{id: input, kind: tensor, config: {addr: 4096, dtype: int8, shape: [4]}}, {id: output, kind: tensor, config: {addr: 4098, dtype: int8, shape: [4]}}, {id: copy, kind: compute, program: {inputs: [A], outputs: [Y], assignments: [{output: Y, axes: [i], expr: {identity: [{read: {tensor: A, indices: [i]}}]}}]}}]\nedges: [{from: input, to: copy, kind: data}, {from: copy, to: output, kind: data}]";
    let error = TimetableFile::from_string(source)
        .unwrap()
        .into_graph()
        .unwrap_err();
    assert!(error.to_string().contains("overlapping range"));
}
fn control_fixture() -> (
    gwr_timetable::TimetableGraph,
    Vec<WordBinding>,
    Vec<EffectClaim>,
    Vec<ControlLinks>,
) {
    let spec = ControlGroup {
        invocation: 9,
        generation: 1,
        device: 7,
        signal: 4224,
        semaphore: 4228,
        participants: 2,
        publisher: 0,
        output_words: vec![4096],
        source_flags: SourceFlags::SplitKHgemmSc0Sc1,
    };
    let links = ControlLinks {
        group: spec.clone(),
        initialization: vec!["init".into()],
        publish: "publish".into(),
        polls: vec!["poll0".into(), "poll1".into()],
        increments: vec!["inc0".into(), "inc1".into()],
        partials: vec!["partial0".into(), "partial1".into()],
        output_updates: vec![vec!["out0".into()], vec!["out1".into()]],
        reset_semaphore: "reset_sem".into(),
        reset_signal: "reset_signal".into(),
        final_join: "join".into(),
    };
    let task_ids = [
        "init",
        "publish",
        "poll0",
        "poll1",
        "inc0",
        "inc1",
        "partial0",
        "partial1",
        "out0",
        "out1",
        "reset_sem",
        "reset_signal",
        "join",
    ];
    let mut source = String::from(
        "nodes:\n- {id: backing, kind: tensor, config: {addr: 4096, dtype: bf16, shape: [2]}}\n- {id: signal, kind: tensor, config: {addr: 4224, dtype: int32, shape: [1]}}\n- {id: semaphore, kind: tensor, config: {addr: 4228, dtype: int32, shape: [1]}}\n",
    );
    for id in task_ids {
        source.push_str(&format!(
            "- {{id: {id}, kind: compute, program: {{inputs: [], outputs: [], assignments: []}}}}\n"
        ));
    }
    source.push_str("edges:\n");
    for (a, b) in [
        ("init", "publish"),
        ("partial0", "poll0"),
        ("partial1", "poll1"),
        ("poll0", "inc0"),
        ("poll1", "inc1"),
        ("inc0", "reset_sem"),
        ("inc1", "reset_sem"),
        ("reset_sem", "reset_signal"),
        ("reset_signal", "join"),
        ("inc0", "out0"),
        ("inc1", "out1"),
        ("init", "out0"),
        ("init", "out1"),
        ("out0", "join"),
        ("out1", "join"),
    ] {
        source.push_str(&format!("- {{from: {a}, to: {b}, kind: dependency}}\n"));
    }
    let graph = TimetableFile::from_string(&source)
        .unwrap()
        .into_graph()
        .unwrap();
    let binding = vec![
        WordBinding {
            device: 7,
            address: 4096,
            tensor: "backing".into(),
        },
        WordBinding {
            device: 7,
            address: 4224,
            tensor: "signal".into(),
        },
        WordBinding {
            device: 7,
            address: 4228,
            tensor: "semaphore".into(),
        },
    ];
    let mut claims = vec![EffectClaim {
        node: "init".into(),
        device: 7,
        address: 4096,
        effect: Effect::InitializeOutput {
            invocation: 9,
            generation: 1,
        },
    }];
    for (node, part, effect, address) in [
        ("publish", 0, ControlEffect::PublishSignalOne, 4224),
        ("poll0", 0, ControlEffect::PollSignal, 4224),
        ("poll1", 1, ControlEffect::PollSignal, 4224),
        ("inc0", 0, ControlEffect::FetchAddOne, 4228),
        ("inc1", 1, ControlEffect::FetchAddOne, 4228),
        ("reset_sem", u32::MAX, ControlEffect::ResetSemaphore, 4228),
        ("reset_signal", u32::MAX, ControlEffect::ResetSignal, 4224),
    ] {
        claims.push(EffectClaim {
            node: node.into(),
            device: 7,
            address,
            effect: Effect::Control {
                invocation: 9,
                generation: 1,
                participant: part,
                effect,
            },
        });
    }
    for (node, participant) in [("out0", 0), ("out1", 1)] {
        let mut c = pair(node);
        if let Effect::Atomic { participant: p, .. } = &mut c.effect {
            *p = participant;
        }
        claims.push(c);
    }
    (graph, binding, claims, vec![links])
}
#[test]
fn complete_actual_control_role_roster_accepts_dynamic_polling_but_rejects_role_mutations() {
    let (graph, bindings, claims, groups) = control_fixture();
    let admitted = admit(&graph, &memories(), &bindings, &claims, &groups).unwrap();
    assert_eq!(admitted.claims().len(), 10);
    let mut mutated = claims.clone();
    if let Effect::Control { generation, .. } = &mut mutated[2].effect {
        *generation = 2;
    }
    assert!(admit(&graph, &memories(), &bindings, &mutated, &groups).is_err());
    let mut missing = groups.clone();
    missing[0].output_updates[1].clear();
    assert!(admit(&graph, &memories(), &bindings, &claims, &missing).is_err());
    let mut publisher = groups.clone();
    publisher[0].publish = "poll0".into();
    assert!(admit(&graph, &memories(), &bindings, &claims, &publisher).is_err());
    let mut early = groups.clone();
    early[0].final_join = "reset_sem".into();
    assert!(admit(&graph, &memories(), &bindings, &claims, &early).is_err());
    let mut ambiguous = groups.clone();
    ambiguous.push(groups[0].clone());
    assert!(admit(&graph, &memories(), &bindings, &claims, &ambiguous).is_err());
}
