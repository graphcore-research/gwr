// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::fmt::Display;
use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::traits::{Routable, SimObject, TotalBytes};
use gwr_engine::types::{AccessType, DeviceId, SimError};
use gwr_track::entity::Entity;
use gwr_track::id::Unique;
use gwr_track::{Id, create_id, track_create_object};

use crate::cache::coherency_manager::CoherenceOp;
use crate::cache::traits::{CoherenceToken, CoherentAccess};
use crate::memory::CacheHintType;
use crate::memory::traits::{AccessMemory, CacheAccess, CacheFillToken, ReadMemory};

#[derive(Clone, Debug)]
pub struct MemoryAccess {
    created_by: Rc<Entity>,
    id: Id,
    access_type: AccessType,
    access_size_bytes: usize,
    dst_addr: u64,
    src_addr: u64,
    dst_device: DeviceId,
    src_device: DeviceId,
    cache_hint: CacheHintType,
    cache_fill_token: Option<CacheFillToken>,
    coherence_op: Option<CoherenceOp>,
    coherence_token: Option<CoherenceToken>,
    #[cfg(feature = "functional")]
    pub(crate) functional: Option<crate::memory::FunctionalPacket>,

    /// Non-data overhead. Control/Read accesses don't contain any data.
    overhead_size_bytes: usize,
}

impl Display for MemoryAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}: {}@{:x}",
            self.access_type, self.access_size_bytes, self.dst_addr
        )
    }
}

impl TotalBytes for MemoryAccess {
    fn payload_bytes(&self) -> Option<usize> {
        Some(self.total_bytes() - self.overhead_size_bytes)
    }

    fn total_bytes(&self) -> usize {
        match self.access_type {
            AccessType::Control
            | AccessType::ReadRequest
            | AccessType::WriteNonPostedResponse
            | AccessType::BarrierRequest
            | AccessType::BarrierResponse => self.overhead_size_bytes,
            AccessType::WriteRequest
            | AccessType::WriteNonPostedRequest
            | AccessType::ReadResponse => self.access_size_bytes + self.overhead_size_bytes,
        }
    }
}

impl Unique for MemoryAccess {
    fn id(&self) -> Id {
        self.id
    }
}

impl AccessMemory for MemoryAccess {
    fn access_size_bytes(&self) -> usize {
        self.access_size_bytes
    }

    fn to_response(&self, mem: &impl ReadMemory) -> Result<Self, SimError> {
        #[cfg(not(feature = "functional"))]
        let _ = mem;
        #[cfg(feature = "functional")]
        if let Some(packet) = &self.functional {
            if !mem.supports_functional()
                || self.coherence_op.is_some()
                || self.coherence_token.is_some()
            {
                return sim_error!(
                    "functional mode does not support caches, atomic memory or coherence"
                );
            }
            let response_type = match self.access_type {
                AccessType::ReadRequest => {
                    self.access_size_bytes
                        .checked_add(self.overhead_size_bytes)
                        .ok_or_else(|| {
                            SimError("read response byte size overflows usize".into())
                        })?;
                    AccessType::ReadResponse
                }
                AccessType::WriteNonPostedRequest => AccessType::WriteNonPostedResponse,
                _ => return sim_error!("unsupported functional packet response"),
            };
            let mut response = self.build_response(response_type);
            response.functional = Some(packet.respond(self.access_type)?);
            return Ok(response);
        }
        if self.access_type != AccessType::ReadRequest {
            return self.to_ack();
        }
        self.access_size_bytes
            .checked_add(self.overhead_size_bytes)
            .ok_or_else(|| SimError("read response byte size overflows usize".into()))?;
        Ok(self.build_response(AccessType::ReadResponse))
    }

    fn to_ack(&self) -> Result<Self, SimError> {
        #[cfg(feature = "functional")]
        if self.functional.is_some() {
            return sim_error!("functional packets require a memory-backed response");
        }
        let response_type = match self.access_type {
            AccessType::Control => AccessType::Control,
            AccessType::WriteNonPostedRequest => AccessType::WriteNonPostedResponse,
            AccessType::BarrierRequest => AccessType::BarrierResponse,
            AccessType::ReadRequest
            | AccessType::ReadResponse
            | AccessType::WriteNonPostedResponse
            | AccessType::BarrierResponse
            | AccessType::WriteRequest => {
                return sim_error!("{}: unsupported by to_ack()", self.access_type);
            }
        };
        Ok(self.build_response(response_type))
    }
}

impl CacheAccess for MemoryAccess {
    fn cache_hint(&self) -> CacheHintType {
        self.cache_hint
    }

    fn cache_read_fragment(
        &self,
        address: u64,
        bytes: usize,
        token: CacheFillToken,
    ) -> Result<Self, SimError> {
        if self.access_type != AccessType::ReadRequest
            || bytes == 0
            || address
                .checked_add(u64::try_from(bytes - 1).map_err(|e| SimError(e.to_string()))?)
                .is_none()
        {
            return Err(SimError("invalid cache fill range/type".into()));
        }
        #[cfg(feature = "functional")]
        if self.functional.is_some() {
            return sim_error!("functional mode does not support cache fragments");
        }
        let mut fragment = Self::new(
            &self.created_by,
            self.access_type,
            bytes,
            address,
            self.src_addr,
            self.dst_device,
            self.src_device,
            self.overhead_size_bytes,
        );
        fragment.cache_hint = self.cache_hint;
        fragment.coherence_op = self.coherence_op;
        fragment.coherence_token = self.coherence_token.clone();
        fragment.cache_fill_token = Some(token);
        Ok(fragment)
    }

    fn cache_fill_token(&self) -> Option<&CacheFillToken> {
        self.cache_fill_token.as_ref()
    }
}

impl CoherentAccess for MemoryAccess {
    fn coherence_op(&self) -> Option<CoherenceOp> {
        self.coherence_op
    }

    fn with_access_type(mut self, access_type: AccessType) -> Self {
        self.access_type = access_type;
        self
    }

    fn with_coherence_op(mut self, op: Option<CoherenceOp>) -> Self {
        self.coherence_op = op;
        self
    }

    fn with_routing(mut self, dst_device_id: DeviceId, src_device_id: DeviceId) -> Self {
        self.dst_device = dst_device_id;
        self.src_device = src_device_id;
        self
    }

    fn coherence_token(&self) -> Option<&CoherenceToken> {
        self.coherence_token.as_ref()
    }

    fn with_coherence_token(mut self, invalidation_token: Option<CoherenceToken>) -> Self {
        self.coherence_token = invalidation_token;
        self
    }

    fn build_coherence_child_request(
        &self,
        access_type: AccessType,
        address: u64,
        bytes: usize,
        fill_token: CacheFillToken,
    ) -> Result<Self, SimError> {
        if bytes == 0
            || u64::try_from(bytes - 1)
                .ok()
                .and_then(|n| address.checked_add(n))
                .is_none()
        {
            return sim_error!("invalid coherence fragment range");
        }
        #[cfg(feature = "functional")]
        if self.functional.is_some() {
            return sim_error!("functional mode does not support coherence fragments");
        }
        let mut child_request = Self::new(
            &self.created_by,
            access_type,
            bytes,
            address,
            self.src_addr,
            self.dst_device,
            self.src_device,
            self.overhead_size_bytes,
        );
        child_request.cache_hint = self.cache_hint;
        child_request.cache_fill_token = Some(fill_token);
        child_request.coherence_op = self.coherence_op;
        child_request.coherence_token = self.coherence_token.clone();
        Ok(child_request)
    }
}

impl Routable for MemoryAccess {
    fn dst_addr(&self) -> u64 {
        self.dst_addr
    }

    fn src_addr(&self) -> u64 {
        self.src_addr
    }

    fn dst_device(&self) -> DeviceId {
        self.dst_device
    }

    fn src_device(&self) -> DeviceId {
        self.src_device
    }

    fn access_type(&self) -> AccessType {
        self.access_type
    }
}

impl MemoryAccess {
    #[must_use]
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        created_by: &Rc<Entity>,
        access_type: AccessType,
        access_size_bytes: usize,
        dst_addr: u64,
        src_addr: u64,
        dst_device: DeviceId,
        src_device: DeviceId,
        overhead_size_bytes: usize,
    ) -> Self {
        let access = Self {
            created_by: created_by.clone(),
            id: create_id!(created_by),
            access_size_bytes,
            access_type,
            dst_addr,
            src_addr,
            dst_device,
            src_device,
            cache_hint: CacheHintType::Allocate,
            cache_fill_token: None,
            coherence_op: None,
            coherence_token: None,
            #[cfg(feature = "functional")]
            functional: None,
            overhead_size_bytes,
        };
        track_create_object!(
            created_by;
            access.id,
            access.total_bytes(),
            "bytes",
            access.access_type() as u8,
            "MemoryAccess: {access}"
        );
        access
    }

    /// Explicit read allocation policy; defaults to Allocate. NoAllocate misses
    /// in the reference cache do not populate or coalesce allocating fills.
    #[must_use]
    pub fn with_cache_hint(mut self, hint: CacheHintType) -> Self {
        self.cache_hint = hint;
        self
    }

    fn build_response(&self, response_type: AccessType) -> Self {
        Self {
            access_type: response_type,
            dst_device: self.src_device,
            src_device: self.dst_device,
            ..self.clone()
        }
    }
}

impl SimObject for MemoryAccess {}

#[cfg(test)]
mod coherence_tests {
    use super::*;
    use crate::cache::traits::CoherentAccess;

    const PARENT_DESTINATION_DEVICE_ID: DeviceId = DeviceId(1);
    const PARENT_SOURCE_DEVICE_ID: DeviceId = DeviceId(2);
    const CHILD_DESTINATION_DEVICE_ID: DeviceId = DeviceId(3);
    const CHILD_SOURCE_DEVICE_ID: DeviceId = DeviceId(4);
    const FILL_ORDINAL: u64 = 7;
    const LINE_SIZE_BYTES: usize = 32;
    const REQUEST_HEADER_SIZE_BYTES: usize = 16;

    #[test]
    fn acks_require_no_memory_and_preserve_request_metadata() {
        // Each ack is a header-only reply to its corresponding
        // request.
        for (request_type, response_type) in [
            (AccessType::Control, AccessType::Control),
            (
                AccessType::WriteNonPostedRequest,
                AccessType::WriteNonPostedResponse,
            ),
            (AccessType::BarrierRequest, AccessType::BarrierResponse),
        ] {
            let request = build_test_request(request_type);
            let ack = request.to_ack().unwrap();

            // Reverse device routing while preserving the range and allocation
            // policy.
            assert_eq!(ack.access_type(), response_type);
            assert_eq!(ack.total_bytes(), REQUEST_HEADER_SIZE_BYTES);
            assert_eq!(ack.dst_device(), request.src_device());
            assert_eq!(ack.src_device(), request.dst_device());
            assert_eq!(ack.dst_addr(), request.dst_addr());
            assert_eq!(ack.src_addr(), request.src_addr());
            assert_eq!(ack.access_size_bytes(), request.access_size_bytes());
            assert_eq!(ack.cache_hint(), request.cache_hint());
            assert_eq!(ack.id(), request.id());

            // Both independent identities and the coherence operation must
            // survive.
            let request_fill_token = request.cache_fill_token().unwrap();
            let ack_fill_token = ack.cache_fill_token().unwrap();
            assert!(Rc::ptr_eq(&ack_fill_token.owner, &request_fill_token.owner));
            assert_eq!(ack_fill_token.ordinal, request_fill_token.ordinal);
            assert!(
                ack.coherence_token()
                    .unwrap()
                    .matches(request.coherence_token().unwrap())
            );
            assert_eq!(ack.coherence_op(), request.coherence_op());
        }
    }

    #[test]
    fn acknowledgements_reject_reads_posted_writes_and_existing_responses() {
        // Reads need the response path; posted writes and responses have no
        // acknowledgement.
        for access_type in [
            AccessType::ReadRequest,
            AccessType::WriteRequest,
            AccessType::ReadResponse,
            AccessType::WriteNonPostedResponse,
            AccessType::BarrierResponse,
        ] {
            let request = build_test_request(access_type);
            assert!(request.to_ack().is_err(), "{access_type}");
        }
    }

    #[test]
    fn read_responses_keep_payload_accounting_and_reject_byte_size_overflow() {
        // Reads still use the memory-backed API and include the payload in wire
        // size.
        let request = build_test_request(AccessType::ReadRequest);
        let response = request.to_response(&TestMemory).unwrap();
        assert_eq!(response.access_type(), AccessType::ReadResponse);
        assert_eq!(
            response.total_bytes(),
            LINE_SIZE_BYTES + REQUEST_HEADER_SIZE_BYTES
        );
        assert_eq!(response.dst_device(), request.src_device());

        // An overflowing payload plus header must fail before constructing the
        // reply.
        let oversized_request = MemoryAccess {
            access_size_bytes: usize::MAX,
            ..request
        };
        assert!(oversized_request.to_response(&TestMemory).is_err());
    }

    #[test]
    fn coherent_child_requests_preserve_fill_identity_routing_and_checked_ranges() {
        // Create a parent at the final valid whole-line address.
        let engine = gwr_engine::engine::Engine::new(&gwr_track::tracker::dev_null_tracker());
        let owner = Rc::new(());
        let token = CacheFillToken {
            owner: owner.clone(),
            ordinal: FILL_ORDINAL,
        };
        let last_line_address = u64::MAX - (LINE_SIZE_BYTES as u64 - 1);
        let read = MemoryAccess::new(
            engine.top(),
            AccessType::ReadRequest,
            LINE_SIZE_BYTES,
            last_line_address,
            0,
            PARENT_DESTINATION_DEVICE_ID,
            PARENT_SOURCE_DEVICE_ID,
            REQUEST_HEADER_SIZE_BYTES,
        );

        // Build a child that carries the supplied fill identity.
        let fragment = read
            .build_coherence_child_request(
                AccessType::WriteNonPostedRequest,
                last_line_address,
                LINE_SIZE_BYTES,
                token.clone(),
            )
            .unwrap();

        // Reject a child range that crosses the address-space boundary.
        assert!(
            read.build_coherence_child_request(
                AccessType::ReadRequest,
                last_line_address,
                LINE_SIZE_BYTES + 1,
                token
            )
            .is_err()
        );

        // Preserve identity and reverse routing when the child responds.
        let response = fragment
            .with_routing(CHILD_DESTINATION_DEVICE_ID, CHILD_SOURCE_DEVICE_ID)
            .to_ack()
            .unwrap();

        // Check the original fill owner, ordinal, range, and destination.
        let returned = response.cache_fill_token().unwrap();
        assert!(Rc::ptr_eq(&returned.owner, &owner));
        assert_eq!(returned.ordinal, FILL_ORDINAL);
        assert_eq!(response.access_size_bytes(), LINE_SIZE_BYTES);
        assert_eq!(response.dst_device(), CHILD_SOURCE_DEVICE_ID);
    }

    fn build_test_request(access_type: AccessType) -> MemoryAccess {
        let engine = gwr_engine::engine::Engine::new(&gwr_track::tracker::dev_null_tracker());
        MemoryAccess::new(
            engine.top(),
            access_type,
            LINE_SIZE_BYTES,
            LINE_SIZE_BYTES as u64,
            0,
            PARENT_DESTINATION_DEVICE_ID,
            PARENT_SOURCE_DEVICE_ID,
            REQUEST_HEADER_SIZE_BYTES,
        )
        .with_cache_hint(CacheHintType::NoAllocate)
        .build_coherence_child_request(
            access_type,
            LINE_SIZE_BYTES as u64,
            LINE_SIZE_BYTES,
            CacheFillToken {
                owner: Rc::new(()),
                ordinal: FILL_ORDINAL,
            },
        )
        .unwrap()
        .with_coherence_op(Some(CoherenceOp::ExclusiveWrite))
        .with_coherence_token(Some(CoherenceToken {
            owner: Rc::new(()),
            ordinal: FILL_ORDINAL,
        }))
    }

    struct TestMemory;

    impl ReadMemory for TestMemory {
        fn read(&self) -> Vec<u8> {
            vec![0; LINE_SIZE_BYTES]
        }
    }
}

#[cfg(all(test, feature = "functional"))]
mod functional_tests {
    use gwr_engine::engine::Engine;
    use gwr_engine::port::{InPort, OutPort};

    use super::*;
    use crate::memory::{FunctionalContext, FunctionalMemory, Memory, MemoryConfig};

    #[test]
    fn nonposted_value_writes_commit_before_legacy_and_service_acknowledgements() {
        for arrival_aware in [false, true] {
            let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
            let clock = engine.default_clock();
            let config = MemoryConfig::new(0, 64, 16, 1);
            let config = if arrival_aware {
                config.with_arrival_aware_service(2)
            } else {
                config
            };
            let memory =
                Memory::new_and_register(&engine, &clock, engine.top(), "memory", config).unwrap();
            let backing = Rc::new(FunctionalMemory::default());
            backing.write_f32(0, &[1.0]).unwrap();
            let context = FunctionalContext::new(backing.clone());
            context.local.write_f32(0, &[7.5]).unwrap();
            let mut request = MemoryAccess::new(
                engine.top(),
                AccessType::WriteNonPostedRequest,
                4,
                0,
                32,
                DeviceId(1),
                DeviceId(2),
                16,
            );
            request.functional = Some(
                context
                    .packet(AccessType::WriteNonPostedRequest, 0, 4)
                    .unwrap(),
            );
            assert!(request.to_ack().is_err());
            assert_eq!(backing.read_f32(0, 1).unwrap(), [1.0]);
            let id = request.id();
            let mut tx = OutPort::new(engine.top(), "requests");
            tx.connect(memory.port_rx()).unwrap();
            let mut rx: InPort<MemoryAccess> =
                InPort::new(&engine, &clock, engine.top(), "responses");
            memory.connect_port_tx(rx.state()).unwrap();
            engine.spawner().spawn(async move {
                tx.put(request)?.await;
                Ok(())
            });
            let received = Rc::new(std::cell::Cell::new(false));
            let received_by_task = received.clone();
            engine.spawner().spawn(async move {
                let response = rx.get()?.await;
                assert_eq!(response.access_type(), AccessType::WriteNonPostedResponse);
                assert_eq!(response.id(), id);
                assert_eq!(response.dst_device(), DeviceId(2));
                assert_eq!(response.total_bytes(), 16);
                assert!(response.functional.is_some());
                assert_eq!(backing.read_f32(0, 1).unwrap(), [7.5]);
                received_by_task.set(true);
                Ok(())
            });
            engine.run().unwrap();
            assert!(received.get());
            assert_eq!(memory.bytes_written(), 4);
        }
    }

    #[test]
    fn value_packets_reject_coherence_children_and_unsupported_endpoints() {
        struct UnsupportedMemory;
        impl ReadMemory for UnsupportedMemory {
            fn read(&self) -> Vec<u8> {
                Vec::new()
            }
        }
        struct ValueMemory;
        impl ReadMemory for ValueMemory {
            fn read(&self) -> Vec<u8> {
                Vec::new()
            }
            fn supports_functional(&self) -> bool {
                true
            }
        }
        let engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
        let backing = Rc::new(FunctionalMemory::default());
        backing.write_f32(0, &[1.0]).unwrap();
        let context = FunctionalContext::new(backing);
        let mut request = MemoryAccess::new(
            engine.top(),
            AccessType::ReadRequest,
            4,
            0,
            32,
            DeviceId(1),
            DeviceId(2),
            16,
        );
        request.functional = Some(context.packet(AccessType::ReadRequest, 0, 4).unwrap());
        assert!(request.to_response(&UnsupportedMemory).is_err());
        let token = CacheFillToken {
            owner: Rc::new(()),
            ordinal: 0,
        };
        assert!(request.cache_read_fragment(0, 4, token.clone()).is_err());
        assert!(
            request
                .build_coherence_child_request(AccessType::ReadRequest, 0, 4, token)
                .is_err()
        );
        let response = request.to_response(&ValueMemory).unwrap();
        response
            .functional
            .unwrap()
            .receive(AccessType::ReadResponse)
            .unwrap();
        assert_eq!(context.local.read_f32(0, 1).unwrap(), [1.0]);
        let coherent = request.with_coherence_op(Some(CoherenceOp::SharedRead));
        assert!(coherent.to_response(&ValueMemory).is_err());
        assert_eq!(coherent.coherence_op(), Some(CoherenceOp::SharedRead));
    }
}
