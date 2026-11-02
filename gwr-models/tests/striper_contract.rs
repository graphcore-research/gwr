// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Stripers accept independent traffic packets without cache or coherence
//! traits.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::traits::{Routable, SimObject, TotalBytes};
use gwr_engine::types::{AccessType, DeviceId, SimError, SimResult};
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::memory::striper::{MemoryStriper, MemoryStriperConfig};
use gwr_models::memory::traits::{AccessMemory, ReadMemory, StripingAccess};
use gwr_track::Id;
use gwr_track::id::Unique;

const STRIPER: DeviceId = DeviceId(9);
const CLIENT: DeviceId = DeviceId(8);
const PARENT_CORRELATION: u64 = 77;
const OPAQUE_TAG: u32 = 0x1234;
const REQUEST_HEADER_BYTES: usize = 5;
const READ_RESPONSE_HEADER_BYTES: usize = 13;
const WRITE_RESPONSE_BYTES: usize = 7;
const FRAGMENTS: [(u64, usize, DeviceId); 4] = [
    (0x2003, 1, DeviceId(1)),
    (0x4000, 4, DeviceId(2)),
    (0x2004, 4, DeviceId(1)),
    (0x4004, 1, DeviceId(2)),
];

// Deliberately neither Default nor a wrapper around MemoryAccess. All packets
// share a tracing ID; fragment IDs must work independently of tracking.
#[derive(Clone, Debug)]
struct TrafficPacket {
    kind: AccessType,
    bytes: usize,
    address: u64,
    correlation: u64,
    destination: DeviceId,
    source: DeviceId,
    wire_bytes: usize,
    opaque_tag: u32,
    opaque_identity: Rc<()>,
}

impl TrafficPacket {
    fn request(kind: AccessType, address: u64, bytes: usize) -> Self {
        Self {
            kind,
            bytes,
            address,
            correlation: PARENT_CORRELATION,
            destination: STRIPER,
            source: CLIENT,
            wire_bytes: Self::wire_bytes(kind, bytes),
            opaque_tag: OPAQUE_TAG,
            opaque_identity: Rc::new(()),
        }
    }

    fn wire_bytes(kind: AccessType, bytes: usize) -> usize {
        match kind {
            AccessType::ReadRequest => REQUEST_HEADER_BYTES,
            AccessType::ReadResponse => READ_RESPONSE_HEADER_BYTES + bytes,
            AccessType::WriteNonPostedResponse => WRITE_RESPONSE_BYTES,
            _ => REQUEST_HEADER_BYTES + bytes,
        }
    }

    fn response(&self, kind: AccessType) -> Self {
        Self {
            kind,
            destination: self.source,
            source: self.destination,
            wire_bytes: Self::wire_bytes(kind, self.bytes),
            ..self.clone()
        }
    }
}

impl fmt::Display for TrafficPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}@{:#x}", self.kind, self.bytes, self.address)
    }
}

impl TotalBytes for TrafficPacket {
    fn total_bytes(&self) -> usize {
        self.wire_bytes
    }
}

impl Unique for TrafficPacket {
    fn id(&self) -> Id {
        Id(0)
    }
}

impl Routable for TrafficPacket {
    fn dst_addr(&self) -> u64 {
        self.address
    }

    fn src_addr(&self) -> u64 {
        self.correlation
    }

    fn dst_device(&self) -> DeviceId {
        self.destination
    }

    fn src_device(&self) -> DeviceId {
        self.source
    }

    fn access_type(&self) -> AccessType {
        self.kind
    }
}

impl SimObject for TrafficPacket {}

impl AccessMemory for TrafficPacket {
    fn access_size_bytes(&self) -> usize {
        self.bytes
    }

    fn to_response(&self, _mem: &impl ReadMemory) -> Result<Self, SimError> {
        // Read responses and header-only write acknowledgements use separate
        // paths.
        if self.kind == AccessType::ReadRequest {
            // This is a traffic-only protocol; no stored read data is needed.
            Ok(self.response(AccessType::ReadResponse))
        } else {
            Err(SimError(format!(
                "unsupported read response: {}",
                self.kind
            )))
        }
    }

    fn to_ack(&self) -> Result<Self, SimError> {
        if self.kind == AccessType::WriteNonPostedRequest {
            Ok(self.response(AccessType::WriteNonPostedResponse))
        } else {
            Err(SimError(format!(
                "unsupported acknowledgement: {}",
                self.kind
            )))
        }
    }
}

impl StripingAccess for TrafficPacket {
    fn stripe_fragment(
        &self,
        address: u64,
        bytes: usize,
        destination: DeviceId,
        source: DeviceId,
        fragment_id: u64,
    ) -> Result<Self, SimError> {
        let last_address = bytes
            .checked_sub(1)
            .and_then(|offset| u64::try_from(offset).ok())
            .and_then(|offset| address.checked_add(offset));
        if !matches!(
            self.kind,
            AccessType::ReadRequest | AccessType::WriteRequest | AccessType::WriteNonPostedRequest
        ) || last_address.is_none()
        {
            return Err(SimError(format!(
                "invalid fragment: kind={}, address={address:#x}, bytes={bytes}",
                self.kind
            )));
        }
        Ok(Self {
            address,
            bytes,
            destination,
            source,
            correlation: fragment_id,
            wire_bytes: Self::wire_bytes(self.kind, bytes),
            ..self.clone()
        })
    }

    fn validate_stripe_response(&self, response: &Self) -> SimResult {
        if response.opaque_tag != self.opaque_tag {
            return Err(SimError(format!(
                "opaque_tag: received {}, expected {}",
                response.opaque_tag, self.opaque_tag
            )));
        }
        if !Rc::ptr_eq(&response.opaque_identity, &self.opaque_identity) {
            return Err(SimError(format!(
                "opaque_identity: received {:p}, expected {:p}",
                Rc::as_ptr(&response.opaque_identity),
                Rc::as_ptr(&self.opaque_identity)
            )));
        }
        let expected_wire_bytes = if self.kind == AccessType::ReadRequest {
            READ_RESPONSE_HEADER_BYTES + self.bytes
        } else {
            WRITE_RESPONSE_BYTES
        };
        if response.wire_bytes != expected_wire_bytes {
            return Err(SimError(format!(
                "wire_bytes: received {}, expected {expected_wire_bytes}",
                response.wire_bytes
            )));
        }
        Ok(())
    }
}

struct NoReadData;

impl ReadMemory for NoReadData {
    fn read(&self) -> Vec<u8> {
        panic!("traffic packets do not fetch read contents")
    }
}

struct Ports {
    input: OutPort<TrafficPacket>,
    output: InPort<TrafficPacket>,
    children: InPort<TrafficPacket>,
    replies: OutPort<TrafficPacket>,
}

fn ports(engine: &mut Engine) -> Ports {
    let clock = engine.default_clock();
    let mut map = MemoryMap::new();
    map.insert(0x2000, 16, DeviceId(1)).unwrap();
    map.insert(0x4000, 16, DeviceId(2)).unwrap();
    let striper = MemoryStriper::<TrafficPacket>::new_and_register(
        engine,
        &clock,
        engine.top(),
        "custom_striper",
        MemoryStriperConfig::new(0x1000, 32, 4, STRIPER, map).with_admission_limits(4, 4),
    )
    .unwrap();
    let mut input = OutPort::new(engine.top(), "input");
    input.connect(striper.port_dev_rx()).unwrap();
    let output = InPort::new(engine, &clock, engine.top(), "output");
    striper.connect_port_dev_tx(output.state()).unwrap();
    let children = InPort::new(engine, &clock, engine.top(), "children");
    striper.connect_port_mem_tx(children.state()).unwrap();
    let mut replies = OutPort::new(engine.top(), "replies");
    replies.connect(striper.port_mem_rx()).unwrap();
    Ports {
        input,
        output,
        children,
        replies,
    }
}

fn reordered_round_trip(kind: AccessType, response_kind: AccessType, wire_bytes: usize) {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let Ports {
        mut input,
        mut output,
        mut children,
        mut replies,
    } = ports(&mut engine);
    let parent = TrafficPacket::request(kind, 0x1003, 10);
    let identity = parent.opaque_identity.clone();
    engine.spawn(async move {
        input.put(parent)?.await;
        Ok(())
    });
    let child_identity = identity.clone();
    engine.spawn(async move {
        let mut fragments = Vec::new();
        for (address, bytes, destination) in FRAGMENTS {
            let child = children.get()?.await;
            assert_eq!(
                (child.address, child.bytes, child.destination),
                (address, bytes, destination)
            );
            assert_eq!(child.kind, kind);
            assert_eq!(child.source, STRIPER);
            assert_eq!(child.opaque_tag, OPAQUE_TAG);
            assert!(Rc::ptr_eq(&child.opaque_identity, &child_identity));
            fragments.push(child);
        }
        let mut fragment_ids = fragments.iter().map(Routable::src_addr).collect::<Vec<_>>();
        fragment_ids.sort_unstable();
        fragment_ids.dedup();
        assert_eq!(fragment_ids.len(), FRAGMENTS.len());
        for child in fragments.into_iter().rev() {
            let response = if child.kind == AccessType::ReadRequest {
                child.to_response(&NoReadData)?
            } else {
                child.to_ack()?
            };
            replies.put(response)?.await;
        }
        Ok(())
    });
    let observed = Rc::new(RefCell::new(None));
    let received = observed.clone();
    engine.spawn(async move {
        *received.borrow_mut() = Some(output.get()?.await);
        Ok(())
    });
    engine.run().unwrap();
    let observed = observed.borrow();
    let response = observed.as_ref().expect("original request must complete");
    assert_eq!(response.kind, response_kind);
    assert_eq!(
        (response.address, response.bytes, response.correlation),
        (0x1003, 10, PARENT_CORRELATION)
    );
    assert_eq!((response.destination, response.source), (CLIENT, STRIPER));
    assert_eq!(response.total_bytes(), wire_bytes);
    assert_eq!(response.opaque_tag, OPAQUE_TAG);
    assert!(Rc::ptr_eq(&response.opaque_identity, &identity));
}

#[test]
fn custom_reads_rejoin_reordered_responses_with_opaque_metadata() {
    reordered_round_trip(AccessType::ReadRequest, AccessType::ReadResponse, 23);
}

#[test]
fn custom_nonposted_writes_rejoin_reordered_acknowledgements() {
    reordered_round_trip(
        AccessType::WriteNonPostedRequest,
        AccessType::WriteNonPostedResponse,
        7,
    );
}

#[test]
fn custom_posted_write_fragments_without_an_upstream_response() {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let Ports {
        mut input,
        mut output,
        mut children,
        mut replies,
    } = ports(&mut engine);
    let posted = TrafficPacket::request(AccessType::WriteRequest, 0x1003, 10);
    let identity = posted.opaque_identity.clone();
    engine.spawn(async move {
        input.put(posted)?.await;
        input
            .put(TrafficPacket::request(AccessType::ReadRequest, 0x101f, 1))?
            .await;
        Ok(())
    });
    engine.spawn(async move {
        for (address, bytes, destination) in FRAGMENTS {
            let child = children.get()?.await;
            assert_eq!(
                (child.address, child.bytes, child.destination),
                (address, bytes, destination)
            );
            assert_eq!(child.kind, AccessType::WriteRequest);
            assert_eq!(child.opaque_tag, OPAQUE_TAG);
            assert!(Rc::ptr_eq(&child.opaque_identity, &identity));
        }
        let read = children.get()?.await;
        assert_eq!(
            (read.address, read.bytes, read.destination),
            (0x400f, 1, DeviceId(2))
        );
        replies.put(read.to_response(&NoReadData)?)?.await;
        Ok(())
    });
    let completed = Rc::new(Cell::new(false));
    let observed = completed.clone();
    engine.spawn(async move {
        let response = output.get()?.await;
        assert_eq!(response.kind, AccessType::ReadResponse);
        assert_eq!(response.address, 0x101f);
        clock.wait_ticks(5)?.await;
        assert!(!output.has_value(), "posted write produced a response");
        observed.set(true);
        Ok(())
    });
    engine.run().unwrap();
    assert!(completed.get(), "following read must complete");
}

#[test]
fn custom_validator_rejects_opaque_metadata_and_wire_size_mismatches() {
    for field in ["opaque_tag", "opaque_identity", "wire_bytes"] {
        let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
        let Ports {
            mut input,
            output,
            mut children,
            mut replies,
        } = ports(&mut engine);
        engine.spawn(async move {
            input
                .put(TrafficPacket::request(AccessType::ReadRequest, 0x1000, 1))?
                .await;
            Ok(())
        });
        engine.spawn(async move {
            let child = children.get()?.await;
            let mut response = child.to_response(&NoReadData)?;
            match field {
                "opaque_tag" => response.opaque_tag = 99,
                "opaque_identity" => response.opaque_identity = Rc::new(()),
                "wire_bytes" => response.wire_bytes = 99,
                _ => unreachable!(),
            }
            replies.put(response)?.await;
            Ok(())
        });
        let error = engine.run().unwrap_err();
        assert!(
            error.0.contains("malformed memory stripe response"),
            "{error}"
        );
        assert!(error.0.contains(field), "{error}");
        assert!(
            !output.has_value(),
            "malformed response completed its parent"
        );
    }
}
