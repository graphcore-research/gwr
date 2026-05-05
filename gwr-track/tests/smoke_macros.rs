// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

//! Ensure that all version of each macro can be used

use std::fmt::Display;
use std::rc::Rc;
use std::time::Duration;

use gwr_track::entity::{Entity, toplevel};
use gwr_track::id::Unique;
use gwr_track::{
    Id, create_id, debug, destroy_id, error, info, set_time, test_helpers, test_init, trace, warn,
};

#[derive(Debug)]
struct TestObject {
    pub id: Id,
}

impl TestObject {
    fn new(entity: &Rc<Entity>, size: usize) -> Self {
        let id = create_id!(entity);
        let object = Self { id };
        entity.track_create_object(id, size, "bytes", u8::MAX, &format!("{object}"));
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

    set_time!(top ; Duration::from_nanos(10));
    test_helpers::check_and_clear(&test_tracker, &["321: set time 10ns"]);
}
