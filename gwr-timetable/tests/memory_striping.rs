// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;

fn platform(mode: &str, cached: bool) -> String {
    let link = if cached {
        "[pe.p, cache.c.dev]"
    } else {
        "[pe.p, 'fabric.f@(0,0)']"
    };
    let cache = if cached {
        "caches:\n - name: c\n   config: {line_size_bytes: 64, num_sets: 64, num_ways: 2, bw_bytes_per_tick: 64, delay_ticks: 1}\n"
    } else {
        ""
    };
    let extra = if cached {
        " - {connect: [cache.c.mem, 'fabric.f@(0,0)']}\n"
    } else {
        ""
    };
    format!(
        r"
operation_classes:
 add: {{operations: [add], arity: 2}}
memory_maps:
 - name: all
   devices: [m0, m1]
   address_mapping: {{mode: {mode}, stripe_bytes: 64, groups: [{{name: hbm, devices: [m0, m1]}}]}}
processing_elements:
 - name: p
   memory_map: all
   config: {{lsu_access_bytes: 64, num_active_requests: 64, compute: {{engines: {{cpu: {{capabilities: [{{class: add, ops_per_tick: 8}}]}}}}}}}}
memories:
 - {{name: m0, kind: hbm, base_address: 0, config: {{capacity_bytes: 4096, bw_bytes_per_tick: 8, delay_ticks: 8}}}}
 - {{name: m1, kind: hbm, base_address: 4096, config: {{capacity_bytes: 4096, bw_bytes_per_tick: 8, delay_ticks: 8}}}}
fabrics:
 - name: f
   kind: functional
   columns: 3
   rows: 1
   config: {{fabric_ports_per_node: 1, port_bits_per_tick: 8192, ticks_per_hop: 1, ticks_overhead: 0}}
{cache}connections:
 - {{connect: {link}}}
{extra} - {{connect: [mem.m0, 'fabric.f@(1,0)']}}
 - {{connect: [mem.m1, 'fabric.f@(2,0)']}}
"
    )
}

const COPY: &str = r"
nodes:
 - {id: A, kind: tensor, config: {addr: 0, dtype: int8, shape: [1024]}}
 - {id: Y, kind: tensor, config: {addr: 4096, dtype: int8, shape: [1024]}}
 - id: copy
   kind: compute
   pe: p
   program:
    inputs: [A]
    outputs: [Y]
    assignments: [{output: Y, axes: [i], expr: {read: {tensor: A, indices: [i]}}}]
edges:
 - {from: A, to: copy, kind: data}
 - {from: copy, to: Y, kind: data}
";

fn run(mode: &str, cached: bool) -> (u64, Vec<(usize, usize)>) {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let p = Rc::new(Platform::from_string(&engine, &clock, &platform(mode, cached)).unwrap());
    let file = TimetableFile::from_string(COPY).unwrap();
    let t = Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &p).unwrap());
    t.preflight().unwrap();
    let dispatch: Rc<dyn Dispatch> = t;
    p.attach_dispatcher(&dispatch);
    engine.run().unwrap();
    assert_eq!(p.pe("p").unwrap().memory_traffic_bytes(), (1024, 1024));
    let bytes = ["m0", "m1"]
        .iter()
        .map(|name| {
            let m = p.memory(name).unwrap();
            (m.bytes_read(), m.bytes_written())
        })
        .collect();
    (clock.time_now_ns() as u64, bytes)
}

#[test]
fn uncached_striping_spreads_real_traffic_and_improves_memory_bound_runtime() {
    let baseline = run("contiguous", false);
    let striped = run("striped", false);
    assert_eq!(baseline.1, vec![(1024, 0), (0, 1024)]);
    assert_eq!(striped.1, vec![(512, 512), (512, 512)]);
    assert!(striped.0 < baseline.0, "{striped:?} vs {baseline:?}");
}

#[test]
fn cached_requests_keep_unique_physical_identities() {
    let (_, traffic) = run("striped", true);
    assert_eq!(traffic, vec![(512, 512), (512, 512)]);
}

#[test]
fn atomic_direct_target_rejected_even_when_first_stripe_is_unchanged() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let p = Platform::from_string(&engine, &clock, &platform("striped", false)).unwrap();
    let error = p
        .pe("p")
        .unwrap()
        .validate_atomic_word_mapping(0, 0)
        .unwrap_err();
    assert!(error.to_string().contains("striped"), "{error}");
    for name in p.memory_names() {
        let m = p.memory(&name).unwrap();
        assert_eq!((m.bytes_read(), m.bytes_written()), (0, 0));
    }
}

#[test]
fn repeated_logical_reads_hit_through_two_cache_levels() {
    let source = platform("striped", true).replace("stripe_bytes: 64", "stripe_bytes: 128")
        .replace(" - {connect: [cache.c.mem, 'fabric.f@(0,0)']}", " - {connect: [cache.c.mem, cache.l2.dev]}\n - {connect: [cache.l2.mem, 'fabric.f@(0,0)']}")
        .replace("connections:", " - name: l2\n   config: {line_size_bytes: 128, num_sets: 32, num_ways: 2, bw_bytes_per_tick: 64, delay_ticks: 1}\nconnections:");
    let mut workload: yaml_serde::Value = yaml_serde::from_str(COPY).unwrap();
    let mut copy = workload["nodes"][2].clone();
    copy["id"] = "copy2".into();
    let mut output = workload["nodes"][1].clone();
    output["id"] = "Z".into();
    output["config"]["addr"] = 6144.into();
    workload["nodes"]
        .as_sequence_mut()
        .unwrap()
        .extend([output, copy]);
    for edge in [
        "{from: A, to: copy2, kind: data}",
        "{from: copy2, to: Z, kind: data}",
        "{from: copy, to: copy2, kind: dependency}",
    ] {
        workload["edges"]
            .as_sequence_mut()
            .unwrap()
            .push(yaml_serde::from_str(edge).unwrap());
    }
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let p = Rc::new(Platform::from_string(&engine, &clock, &source).unwrap());
    let file = TimetableFile::from_string(&yaml_serde::to_string(&workload).unwrap()).unwrap();
    let table = Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &p).unwrap());
    table.preflight().unwrap();
    let dispatch: Rc<dyn Dispatch> = table.clone();
    p.attach_dispatcher(&dispatch);
    engine.run().unwrap();
    table.check_tasks_complete().unwrap();
    for name in ["m0", "m1"] {
        let m = p.memory(name).unwrap();
        assert_eq!((m.bytes_read(), m.bytes_written()), (512, 1024));
    }
    assert_eq!(p.cache("c").unwrap().traffic_snapshot().request_hits, 16);
}

#[test]
fn invalid_output_fails_preflight_before_any_traffic() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let p = Rc::new(Platform::from_string(&engine, &clock, &platform("striped", false)).unwrap());
    let file = TimetableFile::from_string(&COPY.replace("addr: 4096", "addr: 8000")).unwrap();
    let table = Timetable::new(engine.top(), file.into_graph().unwrap(), &p).unwrap();
    assert!(table.preflight().is_err());
    assert_eq!(p.pe("p").unwrap().memory_traffic_bytes(), (0, 0));
    assert_eq!(p.pe("p").unwrap().peak_sram_bytes(), 0);
    for name in ["m0", "m1"] {
        let m = p.memory(name).unwrap();
        assert_eq!((m.bytes_read(), m.bytes_written()), (0, 0));
    }
}

#[test]
fn independent_dma_map_stripes_while_compute_map_remains_contiguous() {
    let mut source: yaml_serde::Value = yaml_serde::from_str(&platform("striped", false)).unwrap();
    let mut compute_map = source["memory_maps"][0].clone();
    compute_map["name"] = "compute".into();
    compute_map
        .as_mapping_mut()
        .unwrap()
        .remove(yaml_serde::Value::from("address_mapping"));
    source["memory_maps"]
        .as_sequence_mut()
        .unwrap()
        .push(compute_map);
    source["processing_elements"][0]["memory_map"] = "compute".into();
    source["processing_elements"][0]["dma_memory_map"] = "all".into();
    source["fabrics"][0]["columns"] = 4.into();
    source["connections"]
        .as_sequence_mut()
        .unwrap()
        .push(yaml_serde::from_str("{connect: [pe.p.dma, 'fabric.f@(3,0)']}").unwrap());
    let mut workload: yaml_serde::Value = yaml_serde::from_str(COPY).unwrap();
    workload["nodes"][2]["kind"] = "transfer".into();
    workload["nodes"][2]["buffer_bytes"] = 128.into();
    workload["nodes"][2]
        .as_mapping_mut()
        .unwrap()
        .remove(yaml_serde::Value::from("program"));
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let p = Rc::new(
        Platform::from_string(&engine, &clock, &yaml_serde::to_string(&source).unwrap()).unwrap(),
    );
    let file = TimetableFile::from_string(&yaml_serde::to_string(&workload).unwrap()).unwrap();
    let table = Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &p).unwrap());
    table.preflight().unwrap();
    let dispatch: Rc<dyn Dispatch> = table.clone();
    p.attach_dispatcher(&dispatch);
    engine.run().unwrap();
    table.check_tasks_complete().unwrap();
    assert_eq!(p.pe("p").unwrap().memory_traffic_bytes(), (1024, 1024));
    for name in ["m0", "m1"] {
        let memory = p.memory(name).unwrap();
        assert_eq!((memory.bytes_read(), memory.bytes_written()), (512, 512));
    }
}
