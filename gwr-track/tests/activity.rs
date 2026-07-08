// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;
use std::io::{BufReader, BufWriter};
use std::rc::Rc;

use gwr_track::entity::{EntityGroup, EntityLane, toplevel};
use gwr_track::trace_visitor::{TraceVisitor, process_capnp};
use gwr_track::tracker::{CapnProtoTracker, EntityManager};
use gwr_track::{Id, Tracker};

#[derive(Default)]
struct ActivityVisitor {
    events: Vec<String>,
}

impl TraceVisitor for ActivityVisitor {
    fn create_lane(&mut self, created_by: Id, id: Id, name: &str) {
        self.events
            .push(format!("{created_by}: created lane {id}, {name}"));
    }

    fn create_group(&mut self, created_by: Id, id: Id, name: &str) {
        self.events
            .push(format!("{created_by}: created group {id}, {name}"));
    }

    fn begin_activity(&mut self, activity: Id, lane: Id, name: &str) {
        self.events
            .push(format!("{activity}: activity begin {name} on lane {lane}"));
    }

    fn add_to_group(&mut self, id: Id, group_id: Id) {
        self.events.push(format!("{id}: added to group {group_id}"));
    }

    fn remove_from_group(&mut self, id: Id, group_id: Id) {
        self.events
            .push(format!("{id}: removed from group {group_id}"));
    }

    fn end_activity(&mut self, id: Id) {
        self.events.push(format!("{id}: activity end"));
    }
}

#[test]
fn activity_events_round_trip_through_capnp_trace() {
    let path = std::env::temp_dir().join(format!("gwr-track-activity-{}.bin", std::process::id()));
    let writer: gwr_track::Writer = Box::new(BufWriter::new(fs::File::create(&path).unwrap()));
    let tracker: Tracker = Rc::new(CapnProtoTracker::new(
        EntityManager::new(log::Level::Trace),
        writer,
    ));

    {
        let top = toplevel(&tracker, "top");
        let mut lane = EntityLane::new(&top, "lane::add");
        let group = EntityGroup::new(&top, "group::add_task");
        lane.begin("add_task (add)");
        lane.end();
        lane.begin_in_group("add_task compute", &group);
        lane.end();
    }
    tracker.shutdown();

    let mut visitor = ActivityVisitor::default();
    let reader = BufReader::new(fs::File::open(&path).unwrap());
    process_capnp(reader, &mut visitor);
    fs::remove_file(path).unwrap();

    assert_eq!(
        visitor.events,
        [
            "2: created lane 3, top::lane::add",
            "2: created group 4, top::group::add_task",
            "5: activity begin add_task (add) on lane 3",
            "5: activity end",
            "6: added to group 4",
            "6: activity begin add_task compute on lane 3",
            "6: activity end",
            "6: removed from group 4",
        ]
    );
}
