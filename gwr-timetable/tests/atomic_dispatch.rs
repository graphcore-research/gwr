// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Source-only native hypotheses for real shared-memory timetable dispatch.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::memory::atomic::*;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::task::Task;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::scheduled::{Release, ScheduledTimetable};
use gwr_timetable::timetable_file::TimetableFile;
use serde_json::json;
fn service() -> AtomicServiceConfig {
    AtomicServiceConfig {
        capacity: 3,
        bytes_per_tick: 64,
        delay_ticks: 0,
        bf16_pair_per_tick: Some(1),
        i32_fetch_add_per_tick: Some(1),
        lane_interleave_bytes: 4,
        lane_bytes_per_tick: vec![64],
    }
}
fn pair(address: u64, participant: u32) -> Effect {
    Effect::Atomic {
        participant,
        word: AtomicWord {
            device: 1,
            address,
            invocation: 99,
            generation: 1,
            operation: AtomicOperation::Bf16PairFadd,
            scope: AtomicScope::Agent,
            ordering: AtomicOrdering::Monotonic,
        },
    }
}
fn platform() -> String {
    format!(
        "operation_classes:\n add: {{operations: [add], arity: 2}}\nmemory_maps:\n - {{name: all, devices: [hbm]}}\nprocessing_elements:\n - name: pe0\n   memory_map: all\n   config:\n    num_active_requests: 4\n    lsu_access_bytes: 4\n    overhead_size_bytes: 16\n    sram_bytes: 64\n    compute: {{engines: {{alu: {{capabilities: [{{class: add, ops_per_tick: 64}}]}}}}}}\nmemories:\n - name: hbm\n   kind: hbm\n   base_address: 4096\n   config:\n    capacity_bytes: 256\n    bw_bytes_per_tick: 64\n    delay_ticks: 0\n    arrival_aware_service: {{max_pending_requests: 3, lanes: {{interleave_bytes: 4, bytes_per_tick: [64]}}}}\n    atomic_service: {}\nconnections:\n - {{connect: [pe.pe0, mem.hbm]}}\n",
        serde_json::to_string(&service()).unwrap()
    )
}
fn file() -> TimetableFile {
    serde_json::from_value(json!({"nodes":[
    {"id":"backing","kind":"tensor","config":{"addr":4096,"dtype":"bf16","shape":[2]}},
    {"id":"A","kind":"tensor","config":{"addr":4096,"dtype":"int8","shape":[4]}},
    {"id":"Y","kind":"tensor","config":{"addr":4200,"dtype":"int8","shape":[4]}},
    {"id":"a","kind":"compute","pe":"pe0","program":{"inputs":[],"outputs":[]}},
    {"id":"b","kind":"compute","pe":"pe0","program":{"inputs":[],"outputs":[]}},
    {"id":"consumer","kind":"compute","pe":"pe0","program":{"inputs":["A"],"outputs":["Y"],"assignments":[{"output":"Y","axes":["i"],"expr":{"read":{"tensor":"A","indices":["i"]}}}]}}
],"edges":[{"from":"a","to":"consumer","kind":"dependency"},{"from":"b","to":"consumer","kind":"dependency"},{"from":"A","to":"consumer","kind":"data"},{"from":"consumer","to":"Y","kind":"data"}],
"atomic_effects":{"memories":[{"device":1,"base":4096,"bytes":256,"path":"cache_free_direct","service":service()}],"bindings":[{"device":1,"address":4096,"tensor":"backing"}],"claims":[{"node":"a","device":1,"address":4096,"effect":pair(4096,0)},{"node":"b","device":1,"address":4096,"effect":pair(4096,1)}],"groups":[]}})).unwrap()
}
struct CountDispatch {
    scheduled: Rc<ScheduledTimetable>,
    active: RefCell<BTreeMap<usize, usize>>,
    completed: RefCell<BTreeMap<usize, usize>>,
}
#[async_trait(?Send)]
impl Dispatch for CountDispatch {
    fn task_by_id(&self, index: usize) -> Result<Task, SimError> {
        self.scheduled.task_by_id(index)
    }
    fn set_task_active(&self, index: usize) -> SimResult {
        *self.active.borrow_mut().entry(index).or_default() += 1;
        self.scheduled.set_task_active(index)
    }
    fn set_task_completed(&self, index: usize) -> SimResult {
        *self.completed.borrow_mut().entry(index).or_default() += 1;
        self.scheduled.set_task_completed(index)
    }
    fn ready_task_indices(&self, pe: &str) -> Result<(bool, Vec<usize>), SimError> {
        self.scheduled.ready_task_indices(pe)
    }
    async fn wait_for_change(&self) {
        self.scheduled.wait_for_change().await
    }
    async fn wait_for_ready(&self, pe: &str) -> SimResult {
        self.scheduled.wait_for_ready(pe).await
    }
    fn total_tasks_for_pe(&self, pe: &str) -> usize {
        self.scheduled.total_tasks_for_pe(pe)
    }
}
#[test]
fn existing_atomic_dispatch_api_requires_a_real_variant_not_structural_empty_program() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform()).unwrap());
    let table = Timetable::new(engine.top(), file().into_graph().unwrap(), &platform).unwrap();
    // Intended API-only RED before the proposed production payloads; after
    // adoption this also proves exact sidecar task resolution, not empty work.
    assert!(matches!(
        table.task_by_id(3).unwrap(),
        Task::AtomicEffect { .. }
    ));
}
#[test]
fn actual_timetable_starts_but_completes_once_only_after_real_reply_ack() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let physical = Rc::new(Platform::from_string(&engine, &clock, &platform()).unwrap());
    let table =
        Rc::new(Timetable::new(engine.top(), file().into_graph().unwrap(), &physical).unwrap());
    for _ in 0..3 {
        table.preflight().unwrap();
        assert!(matches!(
            table.task_by_id(3).unwrap(),
            Task::AtomicEffect { .. }
        ));
    }
    let memory = physical.memory("hbm").unwrap().clone();
    let control = memory.atomic_control().unwrap();
    assert_eq!(control.snapshot().accepted_requests, 0);
    let planned = table.workload_totals().unwrap();
    assert_eq!((planned.read_bytes, planned.written_bytes), (4, 4));
    let scheduled = Rc::new(
        ScheduledTimetable::new(
            table.clone(),
            clock.clone(),
            BTreeMap::from([
                (
                    "a".into(),
                    Release {
                        not_before_tick: 5,
                        ..Default::default()
                    },
                ),
                (
                    "b".into(),
                    Release {
                        not_before_tick: 5,
                        ..Default::default()
                    },
                ),
            ]),
        )
        .unwrap(),
    );
    let counted = Rc::new(CountDispatch {
        scheduled: scheduled.clone(),
        active: RefCell::new(BTreeMap::new()),
        completed: RefCell::new(BTreeMap::new()),
    });
    let dispatcher: Rc<dyn Dispatch> = counted.clone();
    physical.attach_dispatcher(&dispatcher);
    let target = memory.atomic_target().unwrap();
    let slot = Rc::new(RefCell::new(None));
    let pending = slot.clone();
    engine.spawner().spawn(async move {
        *pending.borrow_mut() = Some(target.receive(1, 4100, pair(4100, 0)).await?);
        Ok(())
    });
    let counts = counted.clone();
    let s = scheduled.clone();
    let c = clock.clone();
    engine.spawner().spawn(async move {
        c.wait_ticks(10)?.await;
        let times = s.timings();
        for id in ["a", "b"] {
            assert_eq!(times[id].started_tick, Some(5));
            assert_eq!(times[id].completed_tick, None);
        }
        assert!(counts.completed.borrow().is_empty());
        assert_eq!(control.snapshot().current, 3);
        c.wait_ticks(10)?.await;
        let receipt = slot
            .borrow_mut()
            .take()
            .ok_or_else(|| SimError("stalled real receipt missing".into()))?;
        receipt.acknowledge().await?;
        Ok(())
    });
    engine.run().unwrap();
    table.check_tasks_complete().unwrap();
    let times = scheduled.timings();
    for id in ["a", "b"] {
        assert!(times[id].completed_tick.unwrap() >= 20);
        assert!(times["consumer"].started_tick.unwrap() >= times[id].completed_tick.unwrap());
    }
    assert_eq!(
        counted
            .completed
            .borrow()
            .values()
            .copied()
            .collect::<Vec<_>>(),
        vec![1, 1, 1]
    );
    assert_eq!(physical.pe("pe0").unwrap().memory_traffic_bytes(), (4, 4));
    assert_eq!(table.machine_operations_executed().unwrap(), 0);
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (16, 16));
    let s = memory.atomic_control().unwrap().snapshot();
    assert_eq!((s.current, s.bf16_pair_requests), (0, 3));
    println!("ATOMIC_TIMETABLE_SHARED {s:?} timings={times:?}");
}
#[test]
fn valid_first_region_and_generation_mutations_reject_before_activation_and_intake() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let physical = Rc::new(Platform::from_string(&engine, &clock, &platform()).unwrap());
    let valid = file();
    Timetable::new(engine.top(), valid.clone().into_graph().unwrap(), &physical)
        .unwrap()
        .preflight()
        .unwrap();
    let before = physical
        .memory("hbm")
        .unwrap()
        .atomic_control()
        .unwrap()
        .snapshot();
    let mut wrong = valid.clone();
    let region = &mut wrong.atomic_effects.as_mut().unwrap().memories[0];
    region.service.bytes_per_tick = 32;
    region.service.lane_bytes_per_tick = vec![32];
    assert!(
        Timetable::new(engine.top(), wrong.into_graph().unwrap(), &physical)
            .unwrap_err()
            .to_string()
            .contains("differs from actual")
    );
    let mut wrong = valid.clone();
    wrong.atomic_effects.as_mut().unwrap().memories[0].path = AtomicPath::Cache;
    assert!(
        wrong
            .into_graph()
            .unwrap_err()
            .to_string()
            .contains("cache-free")
    );
    let mut wrong = valid;
    let Effect::Atomic { word, .. } = &mut wrong.atomic_effects.as_mut().unwrap().claims[1].effect
    else {
        panic!()
    };
    word.generation = 2;
    assert!(
        wrong
            .into_graph()
            .unwrap_err()
            .to_string()
            .contains("incompatible unordered")
    );
    assert_eq!(
        physical
            .memory("hbm")
            .unwrap()
            .atomic_control()
            .unwrap()
            .snapshot(),
        before
    );
    assert_eq!(physical.pe("pe0").unwrap().memory_traffic_bytes(), (0, 0));
    assert_eq!(physical.pe("pe0").unwrap().peak_sram_bytes(), 0);
}

#[test]
fn happy_first_actual_pe_map_and_cache_fabric_routes_reject_before_activity() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let good = Rc::new(Platform::from_string(&engine, &clock, &platform()).unwrap());
    Timetable::new(engine.top(), file().into_graph().unwrap(), &good)
        .unwrap()
        .preflight()
        .unwrap();
    let direct = "connections:\n - {connect: [pe.pe0, mem.hbm]}\n";
    let mut unmapped = platform();
    unmapped = unmapped.replace("devices: [hbm]", "devices: []");
    let other=platform().replace("devices: [hbm]","devices: [other]")
        .replace(direct," - name: other\n   kind: hbm\n   base_address: 8192\n   config: {capacity_bytes: 256, bw_bytes_per_tick: 64, delay_ticks: 0}\nconnections:\n - {connect: [pe.pe0, mem.hbm]}\n");
    let cached=platform().replace(direct,"caches:\n - name: c\n   config: {line_size_bytes: 16, num_sets: 16, num_ways: 1, bw_bytes_per_tick: 64, delay_ticks: 0}\nconnections:\n - {connect: [pe.pe0, cache.c.dev]}\n - {connect: [cache.c.mem, mem.hbm]}\n");
    let routed=platform().replace(direct,"fabrics:\n - name: f\n   kind: routed\n   columns: 2\n   rows: 1\n   config: {fabric_ports_per_node: 6, ticks_per_hop: 1, ticks_overhead: 0, rx_buffer_bytes: 64, tx_buffer_bytes: 64, port_bits_per_tick: 64}\nconnections:\n - {connect: [pe.pe0, 'fabric.f@(0,0).0']}\n - {connect: ['fabric.f@(1,0).0', mem.hbm]}\n");
    for (yaml, expected) in [
        (unmapped, "unmapped or maps"),
        (other, "unmapped or maps"),
        (cached, "actual direct ordinary request AND reply"),
        (routed, "actual direct ordinary request AND reply"),
    ] {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let physical = Rc::new(Platform::from_string(&engine, &clock, &yaml).unwrap());
        let before = physical
            .memory("hbm")
            .unwrap()
            .atomic_control()
            .unwrap()
            .snapshot();
        let error =
            Timetable::new(engine.top(), file().into_graph().unwrap(), &physical).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        let pe = physical.pe("pe0").unwrap();
        let raw = gwr_models::processing_element::task::AtomicTaskConfig::new(
            "forged".into(),
            physical.memory("hbm").unwrap().atomic_target().unwrap(),
            1,
            4096,
            pair(4096, 0),
        )
        .unwrap()
        .with_physical_issuer(pe);
        let rejected = pe.preflight_atomic_task(&raw).unwrap_err();
        if expected == "actual direct ordinary request AND reply" {
            assert!(
                rejected
                    .to_string()
                    .contains("actual direct ordinary request AND reply"),
                "{rejected}"
            );
        }
        assert_eq!(
            physical
                .memory("hbm")
                .unwrap()
                .atomic_control()
                .unwrap()
                .snapshot(),
            before
        );
        assert_eq!(physical.pe("pe0").unwrap().memory_traffic_bytes(), (0, 0));
        assert_eq!(physical.pe("pe0").unwrap().peak_sram_bytes(), 0);
        assert_eq!(before.accepted_requests, 0);
    }
}

#[test]
fn actual_pe_word_mapping_checks_unmapped_tail_and_split_device_not_only_start() {
    use gwr_engine::types::DeviceId;
    use gwr_models::memory::memory_map::MemoryMap;
    use gwr_models::processing_element::{ProcessingElement, ProcessingElementConfig};
    let config = ProcessingElementConfig {
        block_execution: None,
        num_active_requests: 2,
        lsu_access_bytes: 4,
        overhead_size_bytes: 0,
        sram_bytes: 64,
        boundary_payload_service: None,
        local_payload_service: None,
        compute: yaml_serde::from_str(
            "engines: {alu: {capabilities: [{class: add, ops_per_tick: 64}]}}",
        )
        .unwrap(),
        operation_classes: yaml_serde::from_str("add: {operations: [add], arity: 2}").unwrap(),
    };
    for bad_tail in [None, Some(DeviceId(2))] {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let mut good = MemoryMap::new();
        good.insert(4096, 4, DeviceId(1)).unwrap();
        let pe = ProcessingElement::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "good",
            &Rc::new(good),
            &config,
            DeviceId(0),
        )
        .unwrap();
        pe.validate_atomic_word_mapping(1, 4096).unwrap();
        let mut map = MemoryMap::new();
        map.insert(4096, 2, DeviceId(1)).unwrap();
        if let Some(device) = bad_tail {
            map.insert(4098, 2, device).unwrap();
        }
        assert_eq!(map.lookup(4096).unwrap().0, DeviceId(1));
        let pe = ProcessingElement::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "bad",
            &Rc::new(map),
            &config,
            DeviceId(0),
        )
        .unwrap();
        assert!(
            pe.validate_atomic_word_mapping(1, 4096)
                .unwrap_err()
                .to_string()
                .contains("unmapped or maps")
        );
        assert_eq!(pe.memory_traffic_bytes(), (0, 0));
        assert_eq!(pe.peak_sram_bytes(), 0);
    }
}

#[test]
fn actual_dynamic_timetable_polls_and_join_use_real_received_values_and_ack_windows() {
    use gwr_models::memory::atomic::{ControlEffect, ControlGroup, SourceFlags};
    let group = ControlGroup {
        invocation: 9,
        generation: 1,
        device: 1,
        signal: 4224,
        semaphore: 4228,
        participants: 2,
        publisher: 0,
        output_words: vec![4096],
        source_flags: SourceFlags::SplitKHgemmSc0Sc1,
    };
    let control = |p, e| Effect::Control {
        invocation: 9,
        generation: 1,
        participant: p,
        effect: e,
    };
    let output = |p| Effect::Atomic {
        participant: p,
        word: AtomicWord {
            device: 1,
            address: 4096,
            invocation: 9,
            generation: 1,
            operation: AtomicOperation::Bf16PairFadd,
            scope: AtomicScope::Agent,
            ordering: AtomicOrdering::Monotonic,
        },
    };
    let mut nodes = vec![
        json!({"id":"C","kind":"tensor","config":{"addr":4096,"dtype":"bf16","shape":[2]}}),
        json!({"id":"signal","kind":"tensor","config":{"addr":4224,"dtype":"int32","shape":[1]}}),
        json!({"id":"semaphore","kind":"tensor","config":{"addr":4228,"dtype":"int32","shape":[1]}}),
    ];
    for id in [
        "init",
        "pub",
        "partial0",
        "partial1",
        "poll0",
        "poll1",
        "fetch0",
        "fetch1",
        "reset_sem",
        "reset_signal",
        "out0",
        "out1",
        "join",
    ] {
        nodes.push(
            json!({"id":id,"kind":"compute","pe":"pe0","program":{"inputs":[],"outputs":[]}}),
        );
    }
    let pairs = [
        ("init", "pub"),
        ("partial0", "poll0"),
        ("partial1", "poll1"),
        ("poll0", "fetch0"),
        ("poll1", "fetch1"),
        ("fetch0", "reset_sem"),
        ("fetch1", "reset_sem"),
        ("reset_sem", "reset_signal"),
        ("reset_signal", "join"),
        ("fetch0", "out0"),
        ("fetch1", "out1"),
        ("init", "out0"),
        ("init", "out1"),
        ("out0", "join"),
        ("out1", "join"),
    ];
    let edges = pairs
        .into_iter()
        .map(|(a, b)| json!({"from":a,"to":b,"kind":"dependency"}))
        .collect::<Vec<_>>();
    let roles = [
        (
            "init",
            4096,
            Effect::InitializeOutput {
                invocation: 9,
                generation: 1,
            },
        ),
        ("pub", 4224, control(0, ControlEffect::PublishSignalOne)),
        ("poll0", 4224, control(0, ControlEffect::PollSignal)),
        ("poll1", 4224, control(1, ControlEffect::PollSignal)),
        ("fetch0", 4228, control(0, ControlEffect::FetchAddOne)),
        ("fetch1", 4228, control(1, ControlEffect::FetchAddOne)),
        (
            "reset_sem",
            4228,
            control(u32::MAX, ControlEffect::ResetSemaphore),
        ),
        (
            "reset_signal",
            4224,
            control(u32::MAX, ControlEffect::ResetSignal),
        ),
        ("out0", 4096, output(0)),
        ("out1", 4096, output(1)),
    ];
    let claims=roles.into_iter().map(|(node,address,effect)|json!({"node":node,"device":1,"address":address,"effect":effect})).collect::<Vec<_>>();
    let file:TimetableFile=serde_json::from_value(json!({"nodes":nodes,"edges":edges,"atomic_effects":{"memories":[{"device":1,"base":4096,"bytes":256,"path":"cache_free_direct","service":service()}],"bindings":[{"device":1,"address":4096,"tensor":"C"},{"device":1,"address":4224,"tensor":"signal"},{"device":1,"address":4228,"tensor":"semaphore"}],"claims":claims,"groups":[{"group":group,"initialization":["init"],"publish":"pub","polls":["poll0","poll1"],"increments":["fetch0","fetch1"],"partials":["partial0","partial1"],"output_updates":[["out0"],["out1"]],"reset_semaphore":"reset_sem","reset_signal":"reset_signal","final_join":"join"}]}})).unwrap();
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let physical = Rc::new(Platform::from_string(&engine, &clock, &platform()).unwrap());
    let table =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &physical).unwrap());
    for _ in 0..3 {
        table.preflight().unwrap();
    }
    assert_eq!(
        physical
            .memory("hbm")
            .unwrap()
            .atomic_control()
            .unwrap()
            .snapshot()
            .accepted_requests,
        0
    );
    let scheduled = Rc::new(
        ScheduledTimetable::new(
            table.clone(),
            clock.clone(),
            BTreeMap::from([(
                "init".into(),
                Release {
                    not_before_tick: 5,
                    ..Default::default()
                },
            )]),
        )
        .unwrap(),
    );
    let counts = Rc::new(CountDispatch {
        scheduled: scheduled.clone(),
        active: RefCell::new(BTreeMap::new()),
        completed: RefCell::new(BTreeMap::new()),
    });
    let dispatcher: Rc<dyn Dispatch> = counts.clone();
    physical.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    table.check_tasks_complete().unwrap();
    let times = scheduled.timings();
    let observed = table.atomic_control_observations();
    assert_eq!(observed.len(), 1);
    let observed = &observed[0];
    let last = observed.last.unwrap();
    assert!(
        observed
            .participants
            .values()
            .all(|p| p.zero_polls > 0 && p.fetched.is_some() && p.output_acks.contains(&4096))
    );
    for id in ["out0", "out1", "reset_signal"] {
        assert!(times["join"].started_tick.unwrap() >= times[id].completed_tick.unwrap());
    }
    assert!(
        times[&format!("out{last}")].completed_tick.unwrap()
            >= times["reset_signal"].completed_tick.unwrap()
    );
    assert_eq!(counts.completed.borrow().len(), 13);
    assert!(counts.completed.borrow().values().all(|n| *n == 1));
    assert_eq!(physical.pe("pe0").unwrap().memory_traffic_bytes(), (0, 0));
    assert_eq!(table.machine_operations_executed().unwrap(), 0);
    let polls = observed.participants.values().map(|p| p.polls).sum::<u64>();
    let memory = physical.memory("hbm").unwrap();
    assert_eq!(
        (memory.bytes_read(), memory.bytes_written()),
        ((16 + 4 * polls) as usize, 32)
    );
    println!("ACTUAL_DYNAMIC_TIMETABLE {observed:?} timings={times:?}");
}
