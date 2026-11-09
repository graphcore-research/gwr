// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::collections::BTreeMap;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::scheduled::{Release, ScheduledTimetable};
use gwr_timetable::timetable_file::TimetableFile;

#[test]
fn releases_wait_for_native_completion_and_idle_without_early_exit() {
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
    let file = TimetableFile::from_string(
        r#"
nodes:
 - {id: A,kind: tensor,config: {addr: 4096,dtype: fp32,shape: [16]}}
 - {id: B,kind: tensor,config: {addr: 8192,dtype: fp32,shape: [16]}}
 - {id: C,kind: tensor,config: {addr: 12288,dtype: fp32,shape: [16]}}
 - {id: first,kind: transfer,pe: pe0,buffer_bytes: 64}
 - {id: second,kind: transfer,pe: pe0,buffer_bytes: 64}
edges:
 - {from: A,to: first,kind: data}
 - {from: first,to: B,kind: data}
 - {from: B,to: second,kind: data}
 - {from: second,to: C,kind: data}
"#,
    )
    .unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let scheduled = Rc::new(
        ScheduledTimetable::new(
            timetable.clone(),
            clock.clone(),
            BTreeMap::from([
                (
                    "first".into(),
                    Release {
                        not_before_tick: 1000,
                        ..Default::default()
                    },
                ),
                (
                    "second".into(),
                    Release {
                        after: vec!["first".into()],
                        delay_ticks: 500,
                        ..Default::default()
                    },
                ),
            ]),
        )
        .unwrap(),
    );
    let dispatcher: Rc<dyn Dispatch> = scheduled.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let times = scheduled.timings();
    assert!(times["first"].started_tick.unwrap() >= 1000);
    assert_eq!(
        times["second"].eligible_tick.unwrap(),
        times["first"].completed_tick.unwrap() + 500
    );
    assert!(times["second"].started_tick.unwrap() >= times["second"].eligible_tick.unwrap());
    assert_eq!(
        clock.tick_now().tick(),
        times["second"].completed_tick.unwrap()
    );
}

#[test]
fn combined_release_cycles_and_unknown_prerequisites_are_rejected() {
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
    let file = TimetableFile::from_string(
        r#"
nodes:
 - {id: A,kind: tensor,config: {addr: 4096,dtype: fp32,shape: [16]}}
 - {id: B,kind: tensor,config: {addr: 8192,dtype: fp32,shape: [16]}}
 - {id: C,kind: tensor,config: {addr: 12288,dtype: fp32,shape: [16]}}
 - {id: first,kind: transfer,pe: pe0,buffer_bytes: 64}
 - {id: second,kind: transfer,pe: pe0,buffer_bytes: 64}
edges:
 - {from: A,to: first,kind: data}
 - {from: first,to: B,kind: data}
 - {from: A,to: second,kind: data}
 - {from: second,to: C,kind: data}
"#,
    )
    .unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    assert!(ScheduledTimetable::new(timetable.clone(), clock.clone(), BTreeMap::new()).is_ok());
    assert!(
        ScheduledTimetable::new(
            timetable.clone(),
            clock.clone(),
            BTreeMap::from([(
                "first".into(),
                Release {
                    after: vec!["missing".into()],
                    ..Default::default()
                }
            )])
        )
        .is_err()
    );
    assert!(
        ScheduledTimetable::new(
            timetable,
            clock,
            BTreeMap::from([
                (
                    "first".into(),
                    Release {
                        after: vec!["second".into()],
                        ..Default::default()
                    }
                ),
                (
                    "second".into(),
                    Release {
                        after: vec!["first".into()],
                        ..Default::default()
                    }
                )
            ])
        )
        .is_err()
    );
}
