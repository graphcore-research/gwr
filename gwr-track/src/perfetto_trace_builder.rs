// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Support for generating Perfetto traces.
//!
//! The public API allows the creation of various TrackDescriptors and
//! TrackEvents, each contained within their own timestamped TracePacket.
//!
//! To create trace files that can be opened using the Perfetto UI the
//! TracePackets must be wrapped in a Trace message. The build_trace_to_bytes()
//! function is provided to do this and serialise the data ready for writing.
//!
//! Multiple TracePackets can included within a single Trace message and
//! multiple trace messages can be written consecutively to the same Perfetto
//! trace file.

use std::collections::HashMap;

use gwr_perfetto::protos::trace_packet::Data;
use gwr_perfetto::protos::track_descriptor::StaticOrDynamicName;
use gwr_perfetto::protos::{
    CounterDescriptor, Trace, TracePacket, TrackDescriptor, TrackEvent, counter_descriptor,
    trace_packet, track_event,
};
use prost::Message;
use rand::random;

use crate::Id;

/// State for a trace builder instance.
pub struct PerfettoTraceBuilder {
    trusted_packet_sequence_id: u32,
    id_to_name: HashMap<u64, String>,
}

impl Default for PerfettoTraceBuilder {
    fn default() -> Self {
        Self {
            trusted_packet_sequence_id: random(),
            id_to_name: HashMap::new(),
        }
    }
}

impl PerfettoTraceBuilder {
    /// Create a Perfetto trace builder.
    ///
    /// Each trace builder instance will use a unique TrustedPacketSequenceId
    /// when creating packets.
    #[must_use]
    pub fn new() -> Self {
        PerfettoTraceBuilder::default()
    }

    fn build_counter_track_descriptor(
        &mut self,
        id: Id,
        parent: Id,
        name: &str,
    ) -> TrackDescriptor {
        let counter_desc = CounterDescriptor {
            r#type: Some(counter_descriptor::BuiltinCounterType::CounterUnspecified as i32),
            unit: Some(counter_descriptor::Unit::Count as i32),
            is_incremental: Some(true),
            ..Default::default()
        };

        let mut track_descriptor = self.build_track_descriptor(id, parent, name);
        track_descriptor.counter = Some(counter_desc);

        track_descriptor
    }

    fn build_value_track_descriptor(&mut self, id: Id, parent: Id, name: &str) -> TrackDescriptor {
        let counter_desc = CounterDescriptor {
            r#type: Some(counter_descriptor::BuiltinCounterType::CounterUnspecified as i32),
            unit: Some(counter_descriptor::Unit::Count as i32),
            is_incremental: Some(false),
            ..Default::default()
        };

        let mut track_descriptor = self.build_track_descriptor(id, parent, name);
        track_descriptor.counter = Some(counter_desc);

        track_descriptor
    }

    fn build_track_descriptor(&mut self, id: Id, parent: Id, name: &str) -> TrackDescriptor {
        self.set_id_to_name(id, name);

        TrackDescriptor {
            uuid: Some(id.0),
            parent_uuid: Some(parent.0),
            static_or_dynamic_name: Some(StaticOrDynamicName::AtraceName(name.to_string())),
            ..Default::default()
        }
    }

    fn set_id_to_name(&mut self, id: Id, name: &str) {
        self.id_to_name.insert(id.0, name.to_owned());
    }

    fn build_counter_track_event(&self, id: Id, other: Id, increment: i64) -> TrackEvent {
        let mut track_event = self.build_track_event(id, other);
        track_event.set_type(track_event::Type::Counter);
        track_event.counter_value_field =
            Some(track_event::CounterValueField::CounterValue(increment));

        track_event
    }

    fn build_value_track_event(&self, id: Id, value: f64) -> TrackEvent {
        let mut track_event = self.build_track_event(id, Id(0));
        track_event.set_type(track_event::Type::Counter);
        track_event.counter_value_field =
            Some(track_event::CounterValueField::DoubleCounterValue(value));

        track_event
    }

    fn build_track_event(&self, id: Id, other: Id) -> TrackEvent {
        TrackEvent {
            track_uuid: Some(id.0),
            name_field: Some(track_event::NameField::Name(self.id_to_name(id, other))),
            ..Default::default()
        }
    }

    fn id_to_name(&self, id: Id, other: Id) -> String {
        let name = match id.0 {
            0 => "root",
            _ => match self.id_to_name.get(&other.0) {
                Some(name) => name,
                None => "UNKNOWN",
            },
        };

        name.to_string()
    }

    /// Build a TracePacket containing the TrackDescriptor for an incremental
    /// counter.
    ///
    /// An incremental counter expects delta value updates.
    #[must_use]
    pub fn build_counter_track_descriptor_trace_packet(
        &mut self,
        current_time_ns: u64,
        id: Id,
        parent: Id,
        name: &str,
    ) -> TracePacket {
        let track_descriptor = self.build_counter_track_descriptor(id, parent, name);

        self.build_track_descriptor_trace_packet(current_time_ns, track_descriptor)
    }

    /// Build a TracePacket containing the TrackDescriptor for a sequence of
    /// values.
    #[must_use]
    pub fn build_value_track_descriptor_trace_packet(
        &mut self,
        current_time_ns: u64,
        id: Id,
        parent: Id,
        name: &str,
    ) -> TracePacket {
        let track_descriptor = self.build_value_track_descriptor(id, parent, name);

        self.build_track_descriptor_trace_packet(current_time_ns, track_descriptor)
    }

    fn build_track_descriptor_trace_packet(
        &self,
        current_time_ns: u64,
        track_descriptor: TrackDescriptor,
    ) -> TracePacket {
        let mut trace_packet = self.build_trace_packet(current_time_ns);
        trace_packet.data = Some(Data::TrackDescriptor(track_descriptor));

        trace_packet
    }

    /// Build a TracePacket containing the TrackEvent for an incremental
    /// counter update.
    #[must_use]
    pub fn build_counter_track_event_trace_packet(
        &self,
        current_time_ns: u64,
        id: Id,
        other: Id,
        increment: i64,
    ) -> TracePacket {
        let track_event = self.build_counter_track_event(id, other, increment);

        self.build_track_event_trace_packet(current_time_ns, track_event)
    }

    /// Build a TracePacket containing the TrackEvent for a floating point
    /// value.
    #[must_use]
    pub fn build_value_track_event_trace_packet(
        &self,
        current_time_ns: u64,
        id: Id,
        value: f64,
    ) -> TracePacket {
        let track_event = self.build_value_track_event(id, value);

        self.build_track_event_trace_packet(current_time_ns, track_event)
    }

    fn build_track_event_trace_packet(
        &self,
        current_time_ns: u64,
        track_event: TrackEvent,
    ) -> TracePacket {
        let mut trace_packet = self.build_trace_packet(current_time_ns);
        trace_packet.data = Some(trace_packet::Data::TrackEvent(track_event));

        trace_packet
    }

    fn build_trace_packet(&self, current_time_ns: u64) -> TracePacket {
        TracePacket {
            timestamp: Some(current_time_ns),
            optional_trusted_packet_sequence_id: Some(
                trace_packet::OptionalTrustedPacketSequenceId::TrustedPacketSequenceId(
                    self.trusted_packet_sequence_id,
                ),
            ),
            ..Default::default()
        }
    }

    /// Build a Trace message containing the passed TracePackets and serialise
    /// it to unsigned bytes.
    #[must_use]
    pub fn build_trace_to_bytes(&self, trace_packets: Vec<TracePacket>) -> Vec<u8> {
        PerfettoTraceBuilder::build_trace(trace_packets).encode_to_vec()
    }

    fn build_trace(trace_packets: Vec<TracePacket>) -> Trace {
        Trace {
            packet: trace_packets,
        }
    }
}
