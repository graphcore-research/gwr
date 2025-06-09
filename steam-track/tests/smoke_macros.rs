// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

//! Ensure that all version of each macro can be used

use std::fmt::Display;
use std::sync::Arc;

use steam_track::entity::{Entity, toplevel};
use steam_track::{
    Tag, create, create_and_track_tag, debug, destroy_tag, enter, error, exit, info, set_time,
    test_helpers, test_init, trace, warn,
};

macro_rules! build_with_entity {
    ($name:ident, $macro:ident, $slvl:expr) => (
        #[test]
        fn $name() {
            let (test_tracker, tracker) = test_init!(100);

            let top = toplevel(&tracker, "top");
            test_helpers::check_and_clear(&test_tracker, &["0: created 100, top, 0, 0 bytes"]);
            assert_eq!(top.tag, Tag(100));

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

    test_helpers::check_and_clear(&test_tracker, &["0: created 10, top, 0, 0 bytes"]);
    assert_eq!(top.tag, Tag(10));

    let tag1 = create_and_track_tag!(top);
    test_helpers::check_and_clear(&test_tracker, &["10: created 11, tag, 0, 0 bytes"]);
    assert_eq!(tag1, Tag(11));

    destroy_tag!(top ; tag1);
    test_helpers::check_and_clear(&test_tracker, &["10: destroyed 11"]);

    drop(top);
    test_helpers::check_and_clear(&test_tracker, &["10: destroyed"]);
}

#[test]
fn enter_exit_basics() {
    let (test_tracker, tracker) = test_init!(40);

    let top = toplevel(&tracker, "top");
    let obj = create_and_track_tag!(top);
    enter!(top ; obj);
    test_helpers::check_and_clear(
        &test_tracker,
        &[
            "0: created 40, top, 0, 0 bytes",
            "40: created 41, tag, 0, 0 bytes",
            "40: 41 entered",
        ],
    );

    exit!(top ; obj);
    test_helpers::check_and_clear(&test_tracker, &["40: 41 exited"]);

    drop(top);
    test_helpers::check_and_clear(&test_tracker, &["40: destroyed"]);
}

#[derive(Debug)]
struct Packet {
    pub tag: Tag,
}

impl Packet {
    fn new(entity: &Arc<Entity>) -> Self {
        let tag = create_and_track_tag!(entity);
        Self { tag }
    }
}

impl Display for Packet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Packet {{ tag: {} }}", self.tag)
    }
}

#[test]
fn num_bytes() {
    let (test_tracker, tracker) = test_init!(121);

    let top = toplevel(&tracker, "top");
    test_helpers::check_and_clear(&test_tracker, &["0: created 121, top, 0, 0 bytes"]);

    let pkt = Packet::new(&top);
    create!(top ; pkt, 10, 0);
    test_helpers::check_and_clear(
        &test_tracker,
        &[
            "121: created 122, tag, 0, 0 bytes",
            r"121: created 122, Packet \{ tag: 122 \}, 0, 10 bytes",
        ],
    );
}

#[test]
fn set_time() {
    let (test_tracker, tracker) = test_init!(321);

    let top = toplevel(&tracker, "top");
    test_helpers::check_and_clear(&test_tracker, &["0: created 321, top, 0, 0 bytes"]);

    set_time!(top ; 10.0);
    test_helpers::check_and_clear(&test_tracker, &["321: set time 10.0ns"]);
}
