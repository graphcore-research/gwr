// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Ensure that all version of each macro can be used

use std::rc::Rc;

use gwr_track::entity::{Entity, toplevel};
use gwr_track::test_helpers::create_tracker;
use gwr_track::tracker::aka::{Aka, populate_aka};

#[test]
fn alternative_names() {
    let tracker = create_tracker(file!());
    let top = toplevel(&tracker, "top");
    let mut aka = Aka::default();
    populate_aka(
        None,
        Some(&mut aka),
        &top,
        &vec![("in", "ingress_0"), ("out", "egress_0")],
    );

    assert_eq!(
        *aka.get_alternative_names("ingress_0").unwrap(),
        vec!["top::in".to_string()]
    );
    assert_eq!(
        *aka.get_alternative_names("egress_0").unwrap(),
        vec!["top::out".to_string()]
    );
    assert!(aka.get_alternative_names("other").is_none());
}

#[test]
fn build_new_aka() {
    let tracker = create_tracker(file!());
    let top = toplevel(&tracker, "top");
    let mut aka = Aka::default();
    populate_aka(
        None,
        Some(&mut aka),
        &top,
        &vec![("in", "ingress_0"), ("out", "egress_0")],
    );

    // Create a new model that is a child of the top
    let model = Rc::new(Entity::new(&top, "model"));

    // And build up a new Aka that just contains "rx", having added a new rename and
    // preserved the existing one
    let mut model_aka = Aka::default();
    populate_aka(
        Some(&aka),
        Some(&mut model_aka),
        &model,
        &vec![("ingress_0", "rx")],
    );
    assert!(model_aka.get_alternative_names("ingress_0").is_none());
    assert!(model_aka.get_alternative_names("egress_0").is_none());
    assert_eq!(
        *model_aka.get_alternative_names("rx").unwrap(),
        vec!["top::in".to_string(), "top::model::ingress_0".to_string()]
    );
}
