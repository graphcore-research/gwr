// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::segmented::{SegmentDispatcher, SegmentRelease};
use gwr_timetable::timetable_file::TimetableFile;

fn setup() -> (Engine, Rc<Platform>, Rc<SegmentDispatcher>) {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Rc::new(
        Platform::from_string(
            &engine,
            &clock,
            include_str!("../examples/compute_region_platform.yaml"),
        )
        .unwrap(),
    );
    let dispatch = Rc::new(SegmentDispatcher::new(platform.clone(), clock, 100));
    let erased: Rc<dyn Dispatch> = dispatch.clone();
    platform.attach_dispatcher(&erased);
    (engine, platform, dispatch)
}
fn segment(
    engine: &Engine,
    platform: &Rc<Platform>,
    name: &str,
    src: u64,
    dst: u64,
) -> Rc<Timetable> {
    let yaml = format!(
        "nodes:\n - {{id: A,kind: tensor,config: {{addr: {src},dtype: fp32,shape: [16]}}}}\n - {{id: B,kind: tensor,config: {{addr: {dst},dtype: fp32,shape: [16]}}}}\n - {{id: {name},kind: transfer,pe: pe0,buffer_bytes: 64}}\nedges:\n - {{from: A,to: {name},kind: data}}\n - {{from: {name},to: B,kind: data}}\n"
    );
    Rc::new(
        Timetable::new(
            engine.top(),
            TimetableFile::from_string(&yaml)
                .unwrap()
                .into_graph()
                .unwrap(),
            platform,
        )
        .unwrap(),
    )
}
#[test]
fn empty_open_queue_accepts_late_work_and_closure_drains() {
    let (mut engine, platform, d) = setup();
    let clock = engine.default_clock();
    let t = segment(&engine, &platform, "late", 4096, 8192);
    let producer = d.clone();
    engine.spawn(async move {
        clock.wait_ticks(1000)?.await;
        producer.publish(t, SegmentRelease::default())?;
        producer.close()?;
        Ok(())
    });
    engine.run().unwrap();
    let times = d.timings();
    assert_eq!(times.len(), 1);
    assert!(times.values().next().unwrap().started_tick.unwrap() >= 1000);
    assert!(d.ready_task_indices("pe0").unwrap().0);
}
#[test]
fn segments_share_resource_and_completion_relative_release() {
    let (mut engine, platform, d) = setup();
    let a = d
        .publish(
            segment(&engine, &platform, "a", 4096, 8192),
            SegmentRelease::default(),
        )
        .unwrap();
    let b = d
        .publish(
            segment(&engine, &platform, "b", 12288, 16384),
            SegmentRelease::default(),
        )
        .unwrap();
    let c = d
        .publish(
            segment(&engine, &platform, "c", 8192, 20480),
            SegmentRelease {
                after: vec![a.tasks["a"]],
                delay_ticks: 500,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(a.tasks["a"] < b.tasks["b"] && b.tasks["b"] < c.tasks["c"]);
    d.close().unwrap();
    engine.run().unwrap();
    let times = d.timings();
    assert_eq!(
        times[&c.tasks["c"]].eligible_tick,
        Some(times[&a.tasks["a"]].completed_tick.unwrap() + 500)
    );
    assert!(
        times[&b.tasks["b"]].completed_tick.unwrap() > times[&a.tasks["a"]].completed_tick.unwrap()
    );
}
#[test]
fn invalid_publication_and_closed_admission_are_atomic() {
    let (engine, platform, d) = setup();
    let a = d
        .publish(
            segment(&engine, &platform, "a", 4096, 8192),
            SegmentRelease::default(),
        )
        .unwrap();
    let before = d.total_tasks_for_pe("pe0");
    assert!(
        d.publish(
            segment(&engine, &platform, "bad", 8192, 12288),
            SegmentRelease::default()
        )
        .is_err()
    );
    assert!(
        d.publish(
            segment(&engine, &platform, "unknown", 16384, 20480),
            SegmentRelease {
                after: vec![usize::MAX],
                ..Default::default()
            }
        )
        .is_err()
    );
    assert_eq!(before, d.total_tasks_for_pe("pe0"));
    let b = d
        .publish(
            segment(&engine, &platform, "b", 16384, 20480),
            SegmentRelease::default(),
        )
        .unwrap();
    assert_eq!(b.tasks["b"], a.tasks["a"] + 3);
    d.close().unwrap();
    assert!(
        d.publish(
            segment(&engine, &platform, "closed", 24576, 28672),
            SegmentRelease::default()
        )
        .is_err()
    );
}
#[test]
fn producer_failure_propagates_through_engine() {
    let (mut engine, _, d) = setup();
    engine.spawn(async move {
        d.fail("source admission failed")?;
        Ok(())
    });
    assert!(
        engine
            .run()
            .unwrap_err()
            .to_string()
            .contains("source admission failed")
    );
}
#[test]
fn readiness_changed_before_wait_registration_is_not_lost() {
    let (mut engine, platform, d) = setup();
    assert!(d.ready_task_indices("pe0").unwrap().1.is_empty());
    d.publish(
        segment(&engine, &platform, "a", 4096, 8192),
        SegmentRelease::default(),
    )
    .unwrap();
    let observer = d.clone();
    engine.spawn(async move {
        observer.wait_for_ready("pe0").await?;
        observer.close()?;
        Ok(())
    });
    engine.run().unwrap();
    assert!(d.ready_task_indices("pe0").unwrap().0);
}
#[test]
fn used_graphs_and_capacity_failure_do_not_consume_ids() {
    let (mut engine, platform, d) = setup();
    let used = segment(&engine, &platform, "used", 4096, 8192);
    used.set_task_active(2).unwrap();
    assert!(d.publish(used, SegmentRelease::default()).is_err());
    let limited = SegmentDispatcher::new(platform.clone(), engine.default_clock(), 2);
    assert!(
        limited
            .publish(
                segment(&engine, &platform, "large", 4096, 8192),
                SegmentRelease::default()
            )
            .is_err()
    );
    assert_eq!(limited.stats().resident_nodes, 0);
    let a = d
        .publish(
            segment(&engine, &platform, "fresh", 4096, 8192),
            SegmentRelease::default(),
        )
        .unwrap();
    assert_eq!(a.tasks["fresh"], 2);
    assert_eq!(d.stats().cumulative_published_tasks, 1);
    assert_eq!(d.stats().resident_tasks, 1);
}
#[test]
fn compute_segments_use_one_native_engine_and_shared_compute_resource() {
    let (mut engine, platform, d) = setup();
    for (id, addr) in [("left", 12288), ("right", 16384)] {
        let yaml = format!(
            r"
nodes:
 - {{id: A,kind: tensor,config: {{addr: 4096,dtype: fp32,shape: [16]}}}}
 - {{id: B,kind: tensor,config: {{addr: 8192,dtype: fp32,shape: [16]}}}}
 - {{id: Y,kind: tensor,config: {{addr: {addr},dtype: fp32,shape: [16]}}}}
 - id: {id}
   kind: compute
   pe: pe0
   program:
     inputs: [A,B]
     outputs: [Y]
     assignments:
      - output: Y
        axes: [i]
        expr: {{add: {{args: [{{read: {{tensor: A,indices: [i]}}}}, {{read: {{tensor: B,indices: [i]}}}}]}}}}
edges:
 - {{from: A,to: {id}.0,kind: data}}
 - {{from: B,to: {id}.1,kind: data}}
 - {{from: {id}.0,to: Y,kind: data}}
"
        );
        let graph = TimetableFile::from_string(&yaml)
            .unwrap()
            .into_graph()
            .unwrap();
        d.publish(
            Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap()),
            SegmentRelease::default(),
        )
        .unwrap();
    }
    d.close().unwrap();
    engine.run().unwrap();
    assert_eq!(
        platform.pe("pe0").unwrap().machine_ops().by_operation["add"],
        32
    );
    assert_eq!(d.stats().completed_tasks, 2);
    assert!(d.timings().values().all(|t| t.started_tick == Some(0)));
    assert!(
        d.timings()
            .values()
            .map(|t| t.completed_tick.unwrap())
            .max()
            .unwrap()
            >= 32
    );
}
#[test]
fn idle_pe_waits_without_spinning_on_another_pes_ready_work() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let mut yaml: yaml_serde::Value =
        yaml_serde::from_str(include_str!("../examples/compute_region_platform.yaml")).unwrap();
    let mut second = yaml["processing_elements"][0].clone();
    second["name"] = "pe1".into();
    yaml["processing_elements"]
        .as_sequence_mut()
        .unwrap()
        .push(second);
    yaml["fabrics"] = yaml_serde::from_str("[{name: switch, kind: functional, columns: 3, rows: 1, config: {port_bits_per_tick: 256, ticks_per_hop: 1, ticks_overhead: 0}}]").unwrap();
    yaml["connections"] = yaml_serde::from_str(r#"[{connect: [pe.pe0, "fabric.switch@(0,0)"]}, {connect: [pe.pe1, "fabric.switch@(1,0)"]}, {connect: [mem.mem, "fabric.switch@(2,0)"]}]"#).unwrap();
    let platform = Rc::new(
        Platform::from_string(&engine, &clock, &yaml_serde::to_string(&yaml).unwrap()).unwrap(),
    );
    let d = Rc::new(SegmentDispatcher::new(platform.clone(), clock.clone(), 100));
    let erased: Rc<dyn Dispatch> = d.clone();
    platform.attach_dispatcher(&erased);
    let file = TimetableFile::from_string("nodes:\n - {id: early,kind: compute,pe: pe1,program: {inputs: [],outputs: []}}\nedges: []\n").unwrap();
    d.publish(
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap()),
        SegmentRelease::default(),
    )
    .unwrap();
    let late = segment(&engine, &platform, "late", 4096, 8192);
    let producer = d.clone();
    engine.spawn(async move {
        clock.wait_ticks(1000)?.await;
        producer.publish(late, SegmentRelease::default())?;
        producer.close()?;
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(d.stats().completed_tasks, 2);
    assert!(d.ready_task_indices("pe0").unwrap().0 && d.ready_task_indices("pe1").unwrap().0);
    assert!(d.timings().values().any(|t| t.started_tick == Some(1000)));
}
#[test]
fn producer_listener_captures_changes_before_future_poll() {
    let (mut engine, _platform, d) = setup();
    let changed = d.subscribe_changes();
    d.close().unwrap();
    engine.spawn(async move {
        changed.await;
        Ok(())
    });
    engine.run().unwrap();
}
#[test]
fn boundary_mapping_is_resolved_before_atomic_publication() {
    let (engine, platform, d) = setup();
    for names in [
        vec!["missing".into()],
        vec!["A".into()],
        vec!["one".into(), "one".into()],
    ] {
        assert!(
            d.publish_boundaries(
                segment(&engine, &platform, "one", 4096, 8192),
                SegmentRelease::default(),
                &names
            )
            .is_err()
        );
        assert_eq!(d.stats().resident_nodes, 0);
    }
    let (published, boundaries) = d
        .publish_boundaries(
            segment(&engine, &platform, "one", 4096, 8192),
            SegmentRelease::default(),
            &["one".into()],
        )
        .unwrap();
    assert_eq!(boundaries, vec![published.tasks["one"]]);
    assert_eq!(boundaries[0], 2);
    assert!(d.timing(boundaries[0]).unwrap().completed_tick.is_none());
    assert!(d.timing(usize::MAX).is_err());
}
#[test]
fn published_handles_reject_other_dispatcher_even_when_ids_collide() {
    let (mut engine, platform, d) = setup();
    let other = SegmentDispatcher::new(platform.clone(), engine.default_clock(), 100);
    let a = d
        .publish(
            segment(&engine, &platform, "a", 4096, 8192),
            SegmentRelease::default(),
        )
        .unwrap();
    let b = other
        .publish(
            segment(&engine, &platform, "b", 4096, 8192),
            SegmentRelease::default(),
        )
        .unwrap();
    assert_eq!(a.tasks["a"], b.tasks["b"]);
    let handle = other.task_handle(b.tasks["b"]).unwrap();
    assert!(d.timing_handle(&handle).is_err());
    assert!(other.timing_handle(&handle).is_ok());
    assert!(d.task_handle(usize::MAX).is_err());
}
#[test]
fn completed_facts_survive_retirement_and_release_later_work_without_numeric_archive() {
    let (mut e, p, d) = setup();
    let first = segment(&e, &p, "first", 4096, 8192);
    let weak = Rc::downgrade(&first);
    let receipt = d.publish(first, SegmentRelease::default()).unwrap();
    let handle = d.task_handle(receipt.tasks["first"]).unwrap();
    assert!(d.completed_fact(&handle).is_err());
    assert!(d.retire_segment(&receipt).is_err());
    let after = segment(&e, &p, "after", 8192, 12288);
    let retry = segment(&e, &p, "retry", 8192, 12288);
    let observer = d.clone();
    e.spawn(async move {
        loop {
            let changed = observer.subscribe_changes();
            if observer.timing_handle(&handle)?.completed_tick.is_some() {
                break;
            }
            changed.await;
        }
        let fact = observer.completed_fact(&handle)?;
        let end = fact.completed_tick();
        assert_eq!(fact.id(), handle.id());
        observer.retire_segment(&receipt)?;
        assert!(weak.upgrade().is_none());
        assert!(observer.timing(handle.id()).is_err());
        assert_eq!(observer.stats().resident_nodes, 0);
        assert!(
            observer
                .publish(
                    retry,
                    SegmentRelease {
                        after: vec![handle.id()],
                        ..Default::default()
                    }
                )
                .is_err()
        );
        let (next, _) = observer.publish_boundaries_after(
            after,
            SegmentRelease {
                delay_ticks: 17,
                ..Default::default()
            },
            &[],
            &[fact.handle().clone()],
        )?;
        let next_handle = observer.task_handle(next.tasks["after"])?;
        loop {
            let changed = observer.subscribe_changes();
            if observer
                .timing_handle(&next_handle)?
                .completed_tick
                .is_some()
            {
                break;
            }
            changed.await;
        }
        assert!(observer.timing_handle(&next_handle)?.started_tick.unwrap() >= end + 17);
        observer.retire_segment(&next)?;
        assert_eq!(observer.stats().resident_tasks, 0);
        assert!(observer.stats().cumulative_published_nodes > 0);
        observer.close()?;
        Ok(())
    });
    e.run().unwrap();
}
#[test]
fn retirement_reuses_resident_budget_for_many_actual_publications() {
    let (mut e, p, _) = setup();
    let d = Rc::new(SegmentDispatcher::new(p.clone(), e.default_clock(), 3));
    let erased: Rc<dyn Dispatch> = d.clone();
    p.attach_dispatcher(&erased);
    let top = e.top().clone();
    let observer = d.clone();
    e.spawn(async move{
        let mut previous=None;let mut last_id=None;
        for index in 0..200 {
            let yaml=format!("nodes:\n - {{id: A,kind: tensor,config: {{addr: 4096,dtype: fp32,shape: [16]}}}}\n - {{id: B,kind: tensor,config: {{addr: 8192,dtype: fp32,shape: [16]}}}}\n - {{id: transfer{index},kind: transfer,pe: pe0,buffer_bytes: 64}}\nedges:\n - {{from: A,to: transfer{index},kind: data}}\n - {{from: transfer{index},to: B,kind: data}}\n");
            let graph=Rc::new(Timetable::new(&top,TimetableFile::from_string(&yaml)?.into_graph()?,&p)?);
            let parents=previous.iter().cloned().collect::<Vec<_>>();let (publication,_)=observer.publish_boundaries_after(graph,SegmentRelease::default(),&[],&parents)?;
            let handle=publication.handles().values().next().unwrap().clone();if let Some(old)=last_id{assert!(handle.id()>old)}last_id=Some(handle.id());
            loop{let changed=observer.subscribe_changes();if observer.completed_fact(&handle).is_ok(){break}changed.await;}
            observer.retire_segment(&publication)?;previous=Some(handle);
            let stats=observer.stats();assert_eq!(stats.resident_nodes,0);assert_eq!(stats.resident_parent_links,0);assert_eq!(stats.resident_physical_accesses,0);assert!(stats.fact_registry_entries<=2);
        }
        assert_eq!(observer.stats().cumulative_published_tasks,200);assert_eq!(observer.stats().cumulative_published_nodes,600);assert_eq!(observer.stats().completed_tasks,200);assert_eq!(observer.stats().peak_resident_nodes,3);observer.close()?;Ok(())
    });
    e.run().unwrap();
}
#[test]
fn retiring_fast_segment_does_not_require_unrelated_native_lane_to_drain() {
    let mut e = Engine::default();
    let clock = e.default_clock();
    let mut yaml: yaml_serde::Value =
        yaml_serde::from_str(include_str!("../examples/compute_region_platform.yaml")).unwrap();
    let mut second = yaml["processing_elements"][0].clone();
    second["name"] = "pe1".into();
    yaml["processing_elements"]
        .as_sequence_mut()
        .unwrap()
        .push(second);
    yaml["fabrics"]=yaml_serde::from_str("[{name: switch, kind: functional, columns: 3, rows: 1, config: {port_bits_per_tick: 256, ticks_per_hop: 1, ticks_overhead: 0}}]").unwrap();
    yaml["connections"]=yaml_serde::from_str(r#"[{connect: [pe.pe0, "fabric.switch@(0,0)"]}, {connect: [pe.pe1, "fabric.switch@(1,0)"]}, {connect: [mem.mem, "fabric.switch@(2,0)"]}]"#).unwrap();
    let p =
        Rc::new(Platform::from_string(&e, &clock, &yaml_serde::to_string(&yaml).unwrap()).unwrap());
    let d = Rc::new(SegmentDispatcher::new(p.clone(), clock, 10));
    let erased: Rc<dyn Dispatch> = d.clone();
    p.attach_dispatcher(&erased);
    let fast=TimetableFile::from_string("nodes:\n - {id: fast,kind: compute,pe: pe0,program: {inputs: [],outputs: []}}\nedges: []\n").unwrap();
    let fast = d
        .publish(
            Rc::new(Timetable::new(e.top(), fast.into_graph().unwrap(), &p).unwrap()),
            SegmentRelease {
                not_before_tick: 10,
                ..Default::default()
            },
        )
        .unwrap();
    let slow=TimetableFile::from_string("nodes:\n - {id: A,kind: tensor,config: {addr: 4096,dtype: fp32,shape: [2048]}}\n - {id: B,kind: tensor,config: {addr: 16384,dtype: fp32,shape: [2048]}}\n - {id: slow,kind: transfer,pe: pe1,buffer_bytes: 64}\nedges:\n - {from: A,to: slow,kind: data}\n - {from: slow,to: B,kind: data}\n").unwrap();
    let slow = d
        .publish(
            Rc::new(Timetable::new(e.top(), slow.into_graph().unwrap(), &p).unwrap()),
            SegmentRelease::default(),
        )
        .unwrap();
    let fast_handle = d.task_handle(fast.tasks["fast"]).unwrap();
    let slow_handle = d.task_handle(slow.tasks["slow"]).unwrap();
    let observer = d.clone();
    e.spawn(async move {
        loop {
            let changed = observer.subscribe_changes();
            if observer.completed_fact(&fast_handle).is_ok() {
                break;
            }
            changed.await;
        }
        assert!(observer.stats().active_tasks > 0);
        assert!(observer.retire_segment(&slow).is_err());
        observer.retire_segment(&fast)?;
        assert_eq!(observer.stats().resident_tasks, 1);
        loop {
            let changed = observer.subscribe_changes();
            if observer.completed_fact(&slow_handle).is_ok() {
                break;
            }
            changed.await;
        }
        observer.retire_segment(&slow)?;
        observer.close()?;
        Ok(())
    });
    e.run().unwrap();
}
