// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::fs::File;
use std::io::{BufReader, Write};
use std::path::Path;

use steam_track::Id;
use steam_track::perfetto_trace_builder::PerfettoTraceBuilder;
use steam_track::trace_visitor::{TraceVisitor, process_capnp};

struct PerfettoGenerator {
    output: File,
    current_time_ns: u64,
    trace_builder: PerfettoTraceBuilder,
}

impl PerfettoGenerator {
    fn new(output: &Path) -> Self {
        Self {
            output: File::create(output)
                .expect("`output` should be a file path that can be written to"),
            current_time_ns: 0,
            trace_builder: PerfettoTraceBuilder::new(),
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

    fn create(&mut self, created_by: Id, id: Id, _num_bytes: usize, _req_type: i8, name: &str) {
        let trace_packet = self
            .trace_builder
            .build_counter_track_descriptor_trace_packet(
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

    fn destroy(&mut self, _destroyed_by: Id, _id: Id) {
        // todo!()
    }

    fn connect(&mut self, _connect_from: Id, _connect_to: Id) {
        // todo!()
    }

    fn enter(&mut self, id: Id, entered: Id) {
        let trace_packet = self.trace_builder.build_counter_track_event_trace_packet(
            self.current_time_ns,
            id,
            entered,
            1,
        );
        let buf = self.trace_builder.build_trace_to_bytes(vec![trace_packet]);
        self.output
            .write_all(&buf)
            .expect("`output` should be writable file");
    }

    fn exit(&mut self, id: Id, exited: Id) {
        let trace_packet = self.trace_builder.build_counter_track_event_trace_packet(
            self.current_time_ns,
            id,
            exited,
            -1,
        );
        let buf = self.trace_builder.build_trace_to_bytes(vec![trace_packet]);
        self.output
            .write_all(&buf)
            .expect("`output` should be writable file");
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
