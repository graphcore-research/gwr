// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::Path;

use gwr_track::Id;
use gwr_track::perfetto_trace_builder::PerfettoTraceBuilder;
use gwr_track::trace_visitor::{TraceVisitor, process_capnp};

struct PerfettoGenerator {
    output: File,
    current_time_ns: u64,
    trace_builder: PerfettoTraceBuilder,
    group_memberships: HashMap<Id, Id>,
    activity_lanes: HashMap<Id, Id>,
}

impl PerfettoGenerator {
    fn new(output: &Path) -> Self {
        Self {
            output: File::create(output)
                .expect("`output` should be a file path that can be written to"),
            current_time_ns: 0,
            trace_builder: PerfettoTraceBuilder::new(),
            group_memberships: HashMap::new(),
            activity_lanes: HashMap::new(),
        }
    }

    fn finish(&mut self) {
        self.output
            .flush()
            .expect("`output` should be a file that can be flushed to");
    }
}

/// The `TraceVisitor` trait is the interface that allows a user to see all the
/// events as a binary file is processed
impl TraceVisitor for PerfettoGenerator {
    fn log(&mut self, _id: Id, _level: log::Level, _message: &str) {
        // todo!()
    }

    fn create_entity(&mut self, created_by: Id, id: Id, name: &str) {
        let trace_packet = self
            .trace_builder
            .build_enter_exit_track_descriptor_trace_packet(
                self.current_time_ns,
                id,
                created_by,
                name,
            );
        let buf = self.trace_builder.build_trace_to_bytes(vec![trace_packet]);
        self.output
            .write_all(&buf)
            .expect("`output` should be writable file");
    }

    fn create_monitor(&mut self, created_by: Id, id: Id, name: &str) {
        let trace_packet = self
            .trace_builder
            .build_value_track_descriptor_trace_packet(self.current_time_ns, id, created_by, name);
        let buf = self.trace_builder.build_trace_to_bytes(vec![trace_packet]);
        self.output
            .write_all(&buf)
            .expect("`output` should be writable file");
    }

    fn create_lane(&mut self, created_by: Id, id: Id, name: &str) {
        let trace_packet = self
            .trace_builder
            .build_activity_track_descriptor_trace_packet(
                self.current_time_ns,
                id,
                created_by,
                name,
            );
        let buf = self.trace_builder.build_trace_to_bytes(vec![trace_packet]);
        self.output
            .write_all(&buf)
            .expect("`output` should be writable file");
    }

    fn create_group(&mut self, _created_by: Id, _id: Id, _name: &str) {}

    fn create_object(
        &mut self,
        created_by: Id,
        id: Id,
        _size: usize,
        _units: &str,
        _req_type: u8,
        details: &str,
    ) {
        let trace_packet = self
            .trace_builder
            .build_enter_exit_track_descriptor_trace_packet(
                self.current_time_ns,
                id,
                created_by,
                details,
            );
        let buf = self.trace_builder.build_trace_to_bytes(vec![trace_packet]);
        self.output
            .write_all(&buf)
            .expect("`output` should be writable file");
    }

    fn destroy(&mut self, _destroyed_by: Id, _id: Id) {
        // todo!()
    }

    fn connect(&mut self, _connect_from: Id, _connect_to: Id) {
        // todo!()
    }

    fn enter(&mut self, id: Id, entered: Id) {
        let trace_packet = self.trace_builder.build_enter_track_event_trace_packet(
            self.current_time_ns,
            id,
            entered,
        );
        let buf = self.trace_builder.build_trace_to_bytes(vec![trace_packet]);
        self.output
            .write_all(&buf)
            .expect("`output` should be writable file");
    }

    fn exit(&mut self, id: Id, exited: Id) {
        let trace_packet = self.trace_builder.build_exit_track_event_trace_packet(
            self.current_time_ns,
            id,
            exited,
        );
        let buf = self.trace_builder.build_trace_to_bytes(vec![trace_packet]);
        self.output
            .write_all(&buf)
            .expect("`output` should be writable file");
    }

    fn value(&mut self, id: Id, value: f64) {
        let trace_packet = self.trace_builder.build_value_track_event_trace_packet(
            self.current_time_ns,
            id,
            value,
        );
        let buf = self.trace_builder.build_trace_to_bytes(vec![trace_packet]);
        self.output
            .write_all(&buf)
            .expect("`output` should be writable file");
    }

    fn add_to_group(&mut self, activity: Id, group_id: Id) {
        self.group_memberships.insert(activity, group_id);
    }

    fn remove_from_group(&mut self, activity: Id, group_id: Id) {
        if self.group_memberships.get(&activity) == Some(&group_id) {
            self.group_memberships.remove(&activity);
        }
    }

    fn begin_activity(&mut self, activity: Id, lane: Id, name: &str) {
        self.activity_lanes.insert(activity, lane);
        let correlation_id = self
            .group_memberships
            .get(&activity)
            .map(|group_id| group_id.0);
        let trace_packet = self.trace_builder.build_activity_begin_trace_packet(
            self.current_time_ns,
            lane,
            name,
            correlation_id,
        );
        let buf = self.trace_builder.build_trace_to_bytes(vec![trace_packet]);
        self.output
            .write_all(&buf)
            .expect("`output` should be writable file");
    }

    fn end_activity(&mut self, activity: Id) {
        if let Some(lane) = self.activity_lanes.remove(&activity) {
            let trace_packet = self
                .trace_builder
                .build_activity_end_trace_packet(self.current_time_ns, lane);
            let buf = self.trace_builder.build_trace_to_bytes(vec![trace_packet]);
            self.output
                .write_all(&buf)
                .expect("`output` should be writable file");
        }
    }

    fn time(&mut self, _id: Id, time_ns: f64) {
        self.current_time_ns = time_ns as u64;
    }
}

pub fn generate_perfetto_trace(input_bin_file_path: &Path, output_file_path: &Path) {
    let file = match File::open(input_bin_file_path) {
        Ok(file) => file,
        Err(e) => {
            println!("Error: {e}");
            return;
        }
    };

    let reader = BufReader::new(file);
    let mut perfetto_gen = PerfettoGenerator::new(output_file_path);
    process_capnp(reader, &mut perfetto_gen);
    perfetto_gen.finish();
}
