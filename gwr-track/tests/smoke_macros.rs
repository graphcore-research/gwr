// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

//! Ensure that all version of each macro can be used

use std::cell::Cell;
use std::fmt::Display;
use std::rc::Rc;

use gwr_track::entity::{Entity, EntityLane, toplevel};
use gwr_track::id::Unique;
use gwr_track::tracker::Tracker;
use gwr_track::tracker::multi_tracker::MultiTracker;
use gwr_track::{
    Id, create_id, debug, destroy_id, error, info, set_time, test_helpers, test_init, trace,
    track_create_object, warn,
};

#[derive(Debug)]
struct TestObject {
    pub id: Id,
}

impl TestObject {
    fn new(entity: &Rc<Entity>, size: usize) -> Self {
        let id = create_id!(entity);
        let object = Self { id };
        track_create_object!(entity ; id, size, "bytes", u8::MAX, "{object}");
        object
    }
}

impl Display for TestObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Object {{ id: {} }}", self.id)
    }
}

impl Unique for TestObject {
    fn id(&self) -> Id {
        self.id
    }
}

macro_rules! build_with_entity {
    ($name:ident, $macro:ident, $slvl:expr) => (
        #[test]
        fn $name() {
            let (test_tracker, tracker) = test_init!(100);

            let top = toplevel(&tracker, "top");
            test_helpers::check_and_clear(&test_tracker, &["0: created entity 100, top"]);
            assert_eq!(top.id, Id(100));

            $macro!(top ; "Loc with no args");
            test_helpers::check_and_clear(&test_tracker, &[concat!("100:", $slvl, ": Loc with no args")]);

            $macro!(top ; "Loc with {} argument", 1);
            test_helpers::check_and_clear(&test_tracker, &[concat!("100:", $slvl, ": Loc with 1 argument")]);

            $macro!(top ; "Loc with {}, {} arguments", 1, 1 + 1);
            test_helpers::check_and_clear(&test_tracker, &[concat!("100:", $slvl,": Loc with 1, 2 arguments")]);

            drop(top);
            test_helpers::check_and_clear(&test_tracker, &["100: destroyed"]);
        }
    );
}

build_with_entity!(trace_with_entity, trace, "TRACE");
build_with_entity!(info_with_entity, info, "INFO");
build_with_entity!(debug_with_entity, debug, "DEBUG");
build_with_entity!(warn_with_entity, warn, "WARN");
build_with_entity!(error_with_entity, error, "ERROR");

fn count_format_evaluation(evaluations: &Cell<usize>) -> &'static str {
    evaluations.set(evaluations.get() + 1);
    "evaluated"
}

#[test]
fn disabled_log_macro_does_not_evaluate_format_arguments() {
    let test_tracker = Rc::new(test_helpers::TestTracker::new(500, log::Level::Error));
    let tracker: Tracker = test_tracker.clone();

    let top = toplevel(&tracker, "top");
    let evaluations = Cell::new(0);

    trace!(top ; "{}", count_format_evaluation(&evaluations));

    assert_eq!(evaluations.get(), 0);
    test_helpers::check_and_clear(&test_tracker, &["0: created entity 500, top"]);
}

#[test]
fn disabled_create_object_macro_does_not_evaluate_format_arguments() {
    let test_tracker = Rc::new(test_helpers::TestTracker::new(550, log::Level::Error));
    let tracker: Tracker = test_tracker.clone();

    let top = toplevel(&tracker, "top");
    let evaluations = Cell::new(0);

    track_create_object!(
        top;
        Id(551),
        1,
        "bytes",
        0,
        "{}",
        count_format_evaluation(&evaluations)
    );

    assert_eq!(evaluations.get(), 0);
    test_helpers::check_and_clear(&test_tracker, &["0: created entity 550, top"]);
}

#[test]
fn log_macro_evaluates_format_arguments_when_any_tracker_enables_level() {
    let trace_tracker = Rc::new(test_helpers::TestTracker::new(600, log::Level::Trace));
    let error_tracker = Rc::new(test_helpers::TestTracker::new(700, log::Level::Error));

    let mut multi_tracker = MultiTracker::default();
    let trace_tracker_dyn: Tracker = trace_tracker.clone();
    let error_tracker_dyn: Tracker = error_tracker.clone();
    multi_tracker.add_tracker(trace_tracker_dyn);
    multi_tracker.add_tracker(error_tracker_dyn);
    let tracker: Tracker = Rc::new(multi_tracker);

    let top = toplevel(&tracker, "top");
    let evaluations = Cell::new(0);

    trace!(top ; "{}", count_format_evaluation(&evaluations));

    assert_eq!(evaluations.get(), 1);
    test_helpers::check_and_clear(
        &trace_tracker,
        &["0: created entity 2, top", "2:TRACE: evaluated"],
    );
    test_helpers::check_and_clear(
        &error_tracker,
        &["0: created entity 2, top", "2:TRACE: evaluated"],
    );
}

#[test]
fn create_destroy() {
    let (test_tracker, tracker) = test_init!(10);

    let top = toplevel(&tracker, "top");

    test_helpers::check_and_clear(&test_tracker, &["0: created entity 10, top"]);
    assert_eq!(top.id, Id(10));

    let obj = TestObject::new(&top, 0);
    test_helpers::check_and_clear(
        &test_tracker,
        &[r"10: created object 11, 255, 0, bytes, Object \{ id: 11 \}"],
    );
    assert_eq!(obj.id, Id(11));

    destroy_id!(top ; obj.id);
    test_helpers::check_and_clear(&test_tracker, &["10: destroyed 11"]);

    drop(top);
    test_helpers::check_and_clear(&test_tracker, &["10: destroyed"]);
}

#[test]
fn enter_exit_basics() {
    let (test_tracker, tracker) = test_init!(40);

    let top = toplevel(&tracker, "top");
    let obj = TestObject::new(&top, 0);
    top.track_enter(obj.id);
    test_helpers::check_and_clear(
        &test_tracker,
        &[
            "0: created entity 40, top",
            r"40: created object 41, 255, 0, bytes, Object \{ id: 41 \}",
            "40: 41 entered",
        ],
    );

    top.track_exit(obj.id);
    test_helpers::check_and_clear(&test_tracker, &["40: 41 exited"]);

    drop(top);
    test_helpers::check_and_clear(&test_tracker, &["40: destroyed"]);
}

#[test]
fn activity_basics() {
    let (test_tracker, tracker) = test_init!(70);

    let top = toplevel(&tracker, "top");
    test_helpers::check_and_clear(&test_tracker, &["0: created entity 70, top"]);

    {
        let mut lane = EntityLane::new(&top, "lane::add");
        test_helpers::check_and_clear(&test_tracker, &["70: created lane 71, top::lane::add"]);

        lane.begin("add_task (add)");
        test_helpers::check_and_clear(
            &test_tracker,
            &["72: activity begin add_task \\(add\\) on lane 71"],
        );

        lane.end();
        test_helpers::check_and_clear(&test_tracker, &["72: activity end"]);
    }
    test_helpers::check_and_clear(&test_tracker, &["70: destroyed 71"]);

    drop(top);
    test_helpers::check_and_clear(&test_tracker, &["70: destroyed"]);
}

#[test]
fn num_bytes() {
    let (test_tracker, tracker) = test_init!(121);

    let top = toplevel(&tracker, "top");
    test_helpers::check_and_clear(&test_tracker, &["0: created entity 121, top"]);

    let _ = TestObject::new(&top, 10);
    test_helpers::check_and_clear(
        &test_tracker,
        &[r"121: created object 122, 255, 10, bytes, Object \{ id: 122 \}"],
    );
}

#[test]
fn set_time() {
    let (test_tracker, tracker) = test_init!(321);

    let top = toplevel(&tracker, "top");
    test_helpers::check_and_clear(&test_tracker, &["0: created entity 321, top"]);

    set_time!(top ; 10.0);
    test_helpers::check_and_clear(&test_tracker, &["321: set time 10.0ns"]);
}
