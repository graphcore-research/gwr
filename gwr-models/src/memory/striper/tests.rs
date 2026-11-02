// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::traits::Routable;
use gwr_engine::types::{AccessType, DeviceId};

use super::*;
use crate::cache::traits::CoherentAccess;
use crate::memory::CacheHintType;
use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::MemoryMap;
use crate::memory::traits::{AccessMemory, CacheAccess, ReadMemory, StripingAccess};

fn config(stripe_size_bytes: usize) -> MemoryStriperConfig {
    let mut map = MemoryMap::new();
    // Deliberately insert in reverse order: the lower address is bank zero.
    map.insert(0x4000, 128, DeviceId(2)).unwrap();
    map.insert(0x2000, 129, DeviceId(1)).unwrap();
    MemoryStriperConfig::new(0x1003, 257, stripe_size_bytes, DeviceId(9), map)
}

#[test]
fn config_checks_per_bank_capacity_and_partial_final_stripes() {
    assert!(config(64).validate().is_ok());
    assert_eq!(config(64).address_range().unwrap(), 0x1003..=0x1103);
    let mut small = MemoryMap::new();
    small.insert(0x2000, 128, DeviceId(1)).unwrap();
    small.insert(0x4000, 128, DeviceId(2)).unwrap();
    assert!(
        MemoryStriperConfig::new(0x1003, 257, 64, DeviceId(9), small)
            .validate()
            .is_err()
    );
    let error = config(0).validate().unwrap_err();
    assert!(error.0.contains("stripe_size_bytes=0"), "{error}");
    for (max_parents, max_fragments, expected) in [
        (0, 1, "max_inflight_parents=0"),
        (1, 0, "max_inflight_fragments=0"),
    ] {
        let error = config(64)
            .with_admission_limits(max_parents, max_fragments)
            .validate()
            .unwrap_err();
        assert!(error.0.contains(expected), "{error}");
    }
    let error = MemoryStriperConfig::new(u64::MAX, 2, 64, DeviceId(9), MemoryMap::new())
        .validate()
        .unwrap_err();
    assert!(
        error.0.contains("base_address=0xffffffffffffffff") && error.0.contains("capacity_bytes=2"),
        "{error}"
    );
    let error = MemoryStriperConfig::new(0, 1, 64, DeviceId(9), MemoryMap::new())
        .validate()
        .unwrap_err();
    assert!(
        error.0.contains("backing memory map is empty") && error.0.contains("device_id=9"),
        "{error}"
    );
}

struct ReplyMemory;
impl ReadMemory for ReplyMemory {
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

#[test]
fn unaligned_read_splits_round_robin_and_rejoins_reordered_responses() {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let striper =
        MemoryStriper::new_and_register(&engine, &clock, engine.top(), "striper", config(64))
            .unwrap();
    let mut input = OutPort::new(engine.top(), "input");
    input.connect(striper.port_dev_rx()).unwrap();
    let mut output = InPort::new(&engine, &clock, engine.top(), "output");
    striper.connect_port_dev_tx(output.state()).unwrap();
    let mut children = InPort::new(&engine, &clock, engine.top(), "children");
    striper.connect_port_mem_tx(children.state()).unwrap();
    let mut replies = OutPort::new(engine.top(), "replies");
    replies.connect(striper.port_mem_rx()).unwrap();
    let parent = MemoryAccess::new(
        engine.top(),
        AccessType::ReadRequest,
        130,
        0x1042,
        77,
        DeviceId(9),
        DeviceId(8),
        16,
    )
    .with_cache_hint(CacheHintType::NoAllocate);
    engine.spawn(async move {
        input.put(parent)?.await;
        Ok(())
    });
    engine.spawn(async move {
        let mut fragments = Vec::new();
        for (address, bytes, device) in [
            (0x203f, 1, 1),
            (0x4000, 64, 2),
            (0x2040, 64, 1),
            (0x4040, 1, 2),
        ] {
            let fragment = children.get()?.await;
            assert_eq!(
                (
                    fragment.dst_addr(),
                    fragment.access_size_bytes(),
                    fragment.dst_device()
                ),
                (address, bytes, DeviceId(device))
            );
            assert_eq!(fragment.src_device(), DeviceId(9));
            assert_eq!(fragment.cache_hint(), CacheHintType::NoAllocate);
            fragments.push(fragment);
        }
        let mut identities = fragments.iter().map(Routable::src_addr).collect::<Vec<_>>();
        identities.sort_unstable();
        identities.dedup();
        assert_eq!(identities.len(), 4);
        for fragment in fragments.into_iter().rev() {
            replies.put(fragment.to_response(&ReplyMemory)?)?.await;
        }
        Ok(())
    });
    let observed = Rc::new(RefCell::new(Vec::new()));
    let received = observed.clone();
    engine.spawn(async move {
        let response = output.get()?.await;
        received.borrow_mut().push(response);
        Ok(())
    });
    engine.run().unwrap();
    let observed = observed.borrow();
    assert_eq!(observed.len(), 1);
    let response = &observed[0];
    assert_eq!(response.access_type(), AccessType::ReadResponse);
    assert_eq!(
        (
            response.dst_addr(),
            response.src_addr(),
            response.access_size_bytes()
        ),
        (0x1042, 77, 130)
    );
    assert_eq!(
        (response.dst_device(), response.src_device()),
        (DeviceId(8), DeviceId(9))
    );
    assert_eq!(response.cache_hint(), CacheHintType::NoAllocate);
}

struct Ports {
    input: OutPort<MemoryAccess>,
    output: InPort<MemoryAccess>,
    children: InPort<MemoryAccess>,
    replies: OutPort<MemoryAccess>,
}

fn ports(engine: &mut Engine, model: &MemoryStriper<MemoryAccess>) -> Ports {
    let clock = engine.default_clock();
    let mut input = OutPort::new(engine.top(), "input");
    input.connect(model.port_dev_rx()).unwrap();
    let output = InPort::new(engine, &clock, engine.top(), "output");
    model.connect_port_dev_tx(output.state()).unwrap();
    let children = InPort::new(engine, &clock, engine.top(), "children");
    model.connect_port_mem_tx(children.state()).unwrap();
    let mut replies = OutPort::new(engine.top(), "replies");
    replies.connect(model.port_mem_rx()).unwrap();
    Ports {
        input,
        output,
        children,
        replies,
    }
}

fn request(engine: &Engine, kind: AccessType, address: u64, bytes: usize) -> MemoryAccess {
    MemoryAccess::new(
        engine.top(),
        kind,
        bytes,
        address,
        77,
        DeviceId(9),
        DeviceId(8),
        16,
    )
}

#[test]
fn one_child_slot_streams_non_power_of_two_stripes_through_last_address() {
    let mut map = MemoryMap::new();
    map.insert(0x2000, 10, DeviceId(1)).unwrap();
    map.insert(u64::MAX - 8, 9, DeviceId(2)).unwrap();
    let config = MemoryStriperConfig::new(u64::MAX - 18, 19, 3, DeviceId(9), map)
        .with_admission_limits(1, 1);
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let model =
        MemoryStriper::new_and_register(&engine, &clock, engine.top(), "striper", config).unwrap();
    let Ports {
        mut input,
        mut output,
        mut children,
        mut replies,
    } = ports(&mut engine, &model);
    let parent = request(
        &engine,
        AccessType::WriteNonPostedRequest,
        u64::MAX - 18,
        19,
    );
    engine.spawn(async move {
        input.put(parent)?.await;
        Ok(())
    });
    let tracked_model = model.clone();
    engine.spawn(async move {
        for (address, bytes) in [
            (0x2000, 3),
            (u64::MAX - 8, 3),
            (0x2003, 3),
            (u64::MAX - 5, 3),
            (0x2006, 3),
            (u64::MAX - 2, 3),
            (0x2009, 1),
        ] {
            let child = children.get()?.await;
            assert_eq!(
                (child.dst_addr(), child.access_size_bytes()),
                (address, bytes)
            );
            assert_eq!(
                tracked_model
                    .worker_state
                    .transactions
                    .borrow()
                    .children
                    .len(),
                1
            );
            replies.put(child.to_ack()?)?.await;
        }
        Ok(())
    });
    let completed = Rc::new(std::cell::Cell::new(false));
    let observed = completed.clone();
    engine.spawn(async move {
        let reply = output.get()?.await;
        assert_eq!(reply.access_type(), AccessType::WriteNonPostedResponse);
        assert_eq!(
            (reply.dst_addr(), reply.access_size_bytes()),
            (u64::MAX - 18, 19)
        );
        observed.set(true);
        Ok(())
    });
    engine.run().unwrap();
    assert!(completed.get());
}

#[test]
fn posted_write_splits_without_upstream_response_and_releases_admission() {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let model = MemoryStriper::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "striper",
        config(64).with_admission_limits(1, 1),
    )
    .unwrap();
    let Ports {
        mut input,
        mut output,
        mut children,
        mut replies,
    } = ports(&mut engine, &model);
    let posted = request(&engine, AccessType::WriteRequest, 0x1042, 2);
    let read = request(&engine, AccessType::ReadRequest, 0x1103, 1);
    engine.spawn(async move {
        input.put(posted)?.await;
        input.put(read)?.await;
        Ok(())
    });
    engine.spawn(async move {
        for (address, kind) in [
            (0x203f, AccessType::WriteRequest),
            (0x4000, AccessType::WriteRequest),
            (0x2080, AccessType::ReadRequest),
        ] {
            let child = children.get()?.await;
            assert_eq!((child.dst_addr(), child.access_type()), (address, kind));
            if kind == AccessType::ReadRequest {
                replies.put(child.to_response(&ReplyMemory)?)?.await;
            }
        }
        Ok(())
    });
    let completed = Rc::new(std::cell::Cell::new(false));
    let observed = completed.clone();
    engine.spawn(async move {
        let response = output.get()?.await;
        assert_eq!(response.access_type(), AccessType::ReadResponse);
        assert_eq!(response.dst_addr(), 0x1103);
        observed.set(true);
        Ok(())
    });
    engine.run().unwrap();
    assert!(completed.get());
    assert!(model.worker_state.transactions.borrow().parents.is_empty());
}

#[test]
fn cache_and_directory_identities_survive_fragmentation_and_joining() {
    use crate::cache::coherency_manager::CoherenceOp;
    use crate::cache::traits::{CoherenceToken, CoherentAccess};
    use crate::memory::traits::CacheFillToken;
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let model =
        MemoryStriper::new_and_register(&engine, &clock, engine.top(), "striper", config(64))
            .unwrap();
    let Ports {
        mut input,
        mut output,
        mut children,
        mut replies,
    } = ports(&mut engine, &model);
    let fill = CacheFillToken {
        owner: Rc::new(()),
        ordinal: 5,
    };
    let token = CoherenceToken {
        owner: Rc::new(()),
        ordinal: 7,
    };
    let parent = request(&engine, AccessType::ReadRequest, 0x1042, 2)
        .cache_read_fragment(0x1042, 2, fill.clone())
        .unwrap()
        .with_coherence_op(Some(CoherenceOp::SharedRead))
        .with_coherence_token(Some(token.clone()));
    engine.spawn(async move {
        input.put(parent)?.await;
        Ok(())
    });
    let child_fill = fill.clone();
    let child_token = token.clone();
    engine.spawn(async move {
        for _ in 0..2 {
            let child = children.get()?.await;
            assert!(Rc::ptr_eq(
                &child.cache_fill_token().unwrap().owner,
                &child_fill.owner
            ));
            assert!(child.coherence_token().unwrap().matches(&child_token));
            assert_eq!(child.coherence_op(), Some(CoherenceOp::SharedRead));
            replies.put(child.to_response(&ReplyMemory)?)?.await;
        }
        Ok(())
    });
    let completed = Rc::new(std::cell::Cell::new(false));
    let observed = completed.clone();
    engine.spawn(async move {
        let response = output.get()?.await;
        let returned_fill = response.cache_fill_token().unwrap();
        assert!(Rc::ptr_eq(&returned_fill.owner, &fill.owner));
        assert_eq!(returned_fill.ordinal, 5);
        assert!(response.coherence_token().unwrap().matches(&token));
        assert_eq!(response.coherence_op(), Some(CoherenceOp::SharedRead));
        observed.set(true);
        Ok(())
    });
    engine.run().unwrap();
    assert!(completed.get());
}

#[test]
fn invalid_requests_fail_before_downstream_traffic() {
    for (kind, address, bytes, device) in [
        (AccessType::ReadRequest, 0x1002, 1, DeviceId(9)),
        (AccessType::ReadRequest, 0x1103, 2, DeviceId(9)),
        (AccessType::ReadRequest, 0x1003, 0, DeviceId(9)),
        (AccessType::ReadRequest, u64::MAX, 2, DeviceId(9)),
        (AccessType::ReadRequest, 0x1003, 1, DeviceId(99)),
        (AccessType::Control, 0x1003, 1, DeviceId(9)),
        (AccessType::BarrierRequest, 0x1003, 1, DeviceId(9)),
        (AccessType::ReadResponse, 0x1003, 1, DeviceId(9)),
    ] {
        let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
        let clock = engine.default_clock();
        let model =
            MemoryStriper::new_and_register(&engine, &clock, engine.top(), "striper", config(64))
                .unwrap();
        let Ports { mut input, .. } = ports(&mut engine, &model);
        let parent = request(&engine, kind, address, bytes).with_routing(device, DeviceId(8));
        engine.spawn(async move {
            input.put(parent)?.await;
            Ok(())
        });
        let error = engine.run().unwrap_err();
        assert!(error.0.contains("striper"), "{error}");
        assert!(
            error.0.contains(&format!("dst_addr={address:#x}"))
                && error.0.contains(&format!("access_size_bytes={bytes}"))
                && error.0.contains(&format!("dst_device={device}")),
            "{error}"
        );
        if kind == AccessType::ReadRequest {
            assert!(
                error.0.contains("expected dst_device=9")
                    && error.0.contains("logical range 0x1003..=0x1103"),
                "{error}"
            );
        } else {
            assert!(
                error.0.contains(&format!("access_type: received {kind}"))
                    && error
                        .0
                        .contains("expected ReadRequest, WriteRequest, or WriteNonPostedRequest"),
                "{error}"
            );
        }
        assert!(model.worker_state.transactions.borrow().parents.is_empty());
        assert!(model.worker_state.transactions.borrow().children.is_empty());
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ResponseCorruption {
    AccessType,
    SourceDevice,
    DestinationDevice,
    Address,
    AccessSize,
    RoutingAndRange,
    RangeAndCacheHint,
    CacheHint,
    CacheFillToken,
    CoherenceOperation,
    CoherenceToken,
    Duplicate,
    TotalBytes,
}

impl ResponseCorruption {
    fn response(
        self,
        child: &MemoryAccess,
        creator: &Rc<gwr_track::entity::Entity>,
    ) -> Result<MemoryAccess, SimError> {
        use crate::cache::coherency_manager::CoherenceOp;
        use crate::cache::traits::CoherenceToken;
        use crate::memory::traits::CacheFillToken;

        Ok(match self {
            Self::AccessType => child
                .clone()
                .with_access_type(AccessType::WriteNonPostedResponse),
            Self::SourceDevice => child
                .to_response(&ReplyMemory)?
                .with_routing(DeviceId(9), DeviceId(99)),
            Self::DestinationDevice => child
                .to_response(&ReplyMemory)?
                .with_routing(DeviceId(99), DeviceId(1)),
            Self::Address => child
                .stripe_fragment(0x2001, 1, DeviceId(1), DeviceId(9), child.src_addr())?
                .to_response(&ReplyMemory)?,
            Self::AccessSize => child
                .stripe_fragment(0x2000, 2, DeviceId(1), DeviceId(9), child.src_addr())?
                .to_response(&ReplyMemory)?,
            Self::RoutingAndRange => child
                .stripe_fragment(0x2001, 1, DeviceId(1), DeviceId(9), child.src_addr())?
                .to_response(&ReplyMemory)?
                .with_routing(DeviceId(9), DeviceId(99)),
            Self::RangeAndCacheHint => child
                .stripe_fragment(0x2001, 1, DeviceId(1), DeviceId(9), child.src_addr())?
                .to_response(&ReplyMemory)?
                .with_cache_hint(CacheHintType::NoAllocate),
            Self::CacheHint => child
                .to_response(&ReplyMemory)?
                .with_cache_hint(CacheHintType::NoAllocate),
            Self::CacheFillToken => child
                .cache_read_fragment(
                    child.dst_addr(),
                    1,
                    CacheFillToken {
                        owner: Rc::new(()),
                        ordinal: 5,
                    },
                )?
                .to_response(&ReplyMemory)?,
            Self::CoherenceOperation => child
                .to_response(&ReplyMemory)?
                .with_coherence_op(Some(CoherenceOp::SharedRead)),
            Self::CoherenceToken => {
                child
                    .to_response(&ReplyMemory)?
                    .with_coherence_token(Some(CoherenceToken {
                        owner: Rc::new(()),
                        ordinal: 7,
                    }))
            }
            Self::TotalBytes => MemoryAccess::new(
                creator,
                AccessType::ReadResponse,
                1,
                child.dst_addr(),
                child.src_addr(),
                DeviceId(9),
                DeviceId(1),
                17,
            ),
            Self::Duplicate => child.to_response(&ReplyMemory)?,
        })
    }

    fn diagnostic(self) -> &'static str {
        match self {
            Self::AccessType => "stripe response type mismatch",
            Self::SourceDevice | Self::DestinationDevice | Self::RoutingAndRange => {
                "stripe response routing mismatch"
            }
            Self::Address | Self::AccessSize | Self::RangeAndCacheHint => {
                "stripe response range mismatch"
            }
            Self::CacheHint | Self::CacheFillToken => "stripe response cache metadata mismatch",
            Self::CoherenceOperation | Self::CoherenceToken => {
                "stripe response coherence metadata mismatch"
            }
            Self::Duplicate => "duplicate",
            Self::TotalBytes => "stripe response wire size mismatch: received 18, expected 17",
        }
    }
}

#[test]
fn malformed_and_duplicate_responses_are_errors() {
    for corrupt in [
        ResponseCorruption::AccessType,
        ResponseCorruption::SourceDevice,
        ResponseCorruption::DestinationDevice,
        ResponseCorruption::Address,
        ResponseCorruption::AccessSize,
        ResponseCorruption::RoutingAndRange,
        ResponseCorruption::RangeAndCacheHint,
        ResponseCorruption::CacheHint,
        ResponseCorruption::CacheFillToken,
        ResponseCorruption::CoherenceOperation,
        ResponseCorruption::CoherenceToken,
        ResponseCorruption::Duplicate,
        ResponseCorruption::TotalBytes,
    ] {
        let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
        let clock = engine.default_clock();
        let model =
            MemoryStriper::new_and_register(&engine, &clock, engine.top(), "striper", config(64))
                .unwrap();
        let Ports {
            mut input,
            mut children,
            mut replies,
            ..
        } = ports(&mut engine, &model);
        let parent = request(&engine, AccessType::ReadRequest, 0x1003, 1);
        engine.spawn(async move {
            input.put(parent)?.await;
            Ok(())
        });
        let creator = engine.top().clone();
        engine.spawn(async move {
            let child = children.get()?.await;
            let response = corrupt.response(&child, &creator)?;
            replies.put(response.clone())?.await;
            if corrupt == ResponseCorruption::Duplicate {
                replies.put(response)?.await;
            }
            Ok(())
        });
        let error = engine.run().unwrap_err();
        assert!(
            error
                .0
                .contains(if corrupt == ResponseCorruption::Duplicate {
                    "duplicate"
                } else {
                    "malformed"
                }),
            "{error}"
        );
        assert!(error.0.contains("striper"), "{corrupt:?}: {error}");
        assert!(error.0.contains("child_id=0"), "{corrupt:?}: {error}");
        assert!(
            error.0.contains(corrupt.diagnostic()),
            "{corrupt:?}: {error}"
        );
        if corrupt != ResponseCorruption::Duplicate {
            let state = model.worker_state.transactions.borrow();
            assert_eq!(state.children.len(), 1, "{corrupt:?}");
            assert!(state.ready_parent_ids.is_empty(), "{corrupt:?}");
        }
    }
}

#[test]
fn parent_and_fragment_limits_apply_independently() {
    for (max_parents, max_fragments) in [(1, 4), (4, 1)] {
        for task_order_seed in [None, Some(0), Some(1), Some(17), Some(42)] {
            run_separate_limits_case(max_parents, max_fragments, task_order_seed);
        }
    }
}

fn run_separate_limits_case(
    max_parents: usize,
    max_fragments: usize,
    task_order_seed: Option<u64>,
) {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    engine.set_randomize_task_order(task_order_seed.is_some());
    if let Some(seed) = task_order_seed {
        engine.set_task_order_seed(seed);
    }
    let clock = engine.default_clock();
    let settings = config(4).with_admission_limits(max_parents, max_fragments);
    let model = MemoryStriper::new_and_register(&engine, &clock, engine.top(), "striper", settings)
        .unwrap();
    let Ports {
        mut input,
        mut output,
        mut children,
        mut replies,
    } = ports(&mut engine, &model);
    let parents = (0..=max_parents)
        .map(|index| {
            request(
                &engine,
                AccessType::ReadRequest,
                0x1003 + index as u64 * 16,
                16,
            )
        })
        .collect::<Vec<_>>();
    let admitted = Rc::new(std::cell::Cell::new(0));
    let observed_admissions = admitted.clone();
    let input_clock = clock.clone();
    engine.spawn(async move {
        for (index, parent) in parents.into_iter().enumerate() {
            input.put(parent)?.await;
            observed_admissions.set(index + 1);
            if index == max_parents {
                assert!(
                    input_clock.tick_now().tick() >= 20,
                    "parents must retain admission until accepted upstream"
                );
            }
        }
        Ok(())
    });
    let limits_observed = Rc::new(std::cell::Cell::new(false));
    let observed_limits = limits_observed.clone();
    let memory_clock = clock.clone();
    let observed_admissions = admitted.clone();
    engine.spawn(async move {
        let mut held_fragments = Vec::new();
        for _ in 0..max_fragments {
            held_fragments.push(children.get()?.await);
        }
        memory_clock.wait_ticks(5)?.await;
        assert_eq!(observed_admissions.get(), max_parents);
        assert!(
            !children.has_value(),
            "fragment issuance exceeded its independent limit"
        );
        observed_limits.set(true);
        for fragment in held_fragments {
            replies.put(fragment.to_response(&ReplyMemory)?)?.await;
        }
        for _ in max_fragments..(max_parents + 1) * 4 {
            let fragment = children.get()?.await;
            replies.put(fragment.to_response(&ReplyMemory)?)?.await;
        }
        Ok(())
    });
    let observed_limits = limits_observed.clone();
    let probe_clock = clock.clone();
    engine.spawn(async move {
        probe_clock.wait_ticks(6)?.await;
        assert!(
            observed_limits.get(),
            "one parent must be able to use all configured fragment slots"
        );
        Ok(())
    });
    let completed = Rc::new(std::cell::Cell::new(0));
    let observed_completions = completed.clone();
    engine.spawn(async move {
        clock.wait_ticks(20)?.await;
        let mut addresses = Vec::new();
        for _ in 0..=max_parents {
            let response = output.get()?.await;
            assert_eq!(response.access_type(), AccessType::ReadResponse);
            assert_eq!(response.access_size_bytes(), 16);
            addresses.push(response.dst_addr());
        }
        addresses.sort_unstable();
        assert_eq!(
            addresses,
            (0..=max_parents)
                .map(|index| 0x1003 + index as u64 * 16)
                .collect::<Vec<_>>()
        );
        observed_completions.set(addresses.len());
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(admitted.get(), max_parents + 1);
    assert_eq!(completed.get(), max_parents + 1);
}

#[test]
fn blocked_outputs_do_not_block_response_ingress_or_release_parent_admission_early() {
    for task_order_seed in [None, Some(0), Some(1), Some(17), Some(42)] {
        run_blocked_outputs_case(task_order_seed);
    }
}

fn run_blocked_outputs_case(task_order_seed: Option<u64>) {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    engine.set_randomize_task_order(task_order_seed.is_some());
    if let Some(seed) = task_order_seed {
        engine.set_task_order_seed(seed);
    }
    let clock = engine.default_clock();
    let model = MemoryStriper::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "striper",
        config(64).with_admission_limits(3, 3),
    )
    .unwrap();
    let Ports {
        mut input,
        mut output,
        mut children,
        mut replies,
    } = ports(&mut engine, &model);
    let parents = (0..4)
        .map(|offset| request(&engine, AccessType::ReadRequest, 0x1003 + offset, 1))
        .collect::<Vec<_>>();
    let input_clock = clock.clone();
    let fourth_admitted = Rc::new(std::cell::Cell::new(false));
    let admitted = fourth_admitted.clone();
    engine.spawn(async move {
        for (index, parent) in parents.into_iter().enumerate() {
            input.put(parent)?.await;
            if index == 3 {
                assert!(
                    input_clock.tick_now().tick() >= 20,
                    "ready parents must retain admission until accepted upstream"
                );
                admitted.set(true);
            }
        }
        Ok(())
    });
    let responses_accepted = Rc::new(std::cell::Cell::new(false));
    let accepted = responses_accepted.clone();
    let memory_clock = clock.clone();
    engine.spawn(async move {
        let first = children.get()?.await;
        let second = children.get()?.await;
        memory_clock.wait_ticks(5)?.await;
        // The upstream output and third downstream send are both blocked.
        replies.put(second.to_response(&ReplyMemory)?)?.await;
        replies.put(first.to_response(&ReplyMemory)?)?.await;
        assert_eq!(memory_clock.tick_now().tick(), 5);
        accepted.set(true);
        memory_clock.wait_ticks(5)?.await;
        let third = children.get()?.await;
        replies.put(third.to_response(&ReplyMemory)?)?.await;
        let fourth = children.get()?.await;
        replies.put(fourth.to_response(&ReplyMemory)?)?.await;
        Ok(())
    });
    let observed = responses_accepted.clone();
    let probe_clock = clock.clone();
    engine.spawn(async move {
        probe_clock.wait_ticks(6)?.await;
        assert!(observed.get());
        Ok(())
    });
    let completed = Rc::new(std::cell::Cell::new(0));
    let observed = completed.clone();
    engine.spawn(async move {
        clock.wait_ticks(20)?.await;
        let mut addresses = Vec::new();
        for _ in 0..4 {
            addresses.push(output.get()?.await.dst_addr());
        }
        addresses.sort_unstable();
        assert_eq!(addresses, [0x1003, 0x1004, 0x1005, 0x1006]);
        observed.set(addresses.len());
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(completed.get(), 4);
    assert!(fourth_admitted.get());
}

#[test]
fn a_read_at_last_logical_and_backing_address_completes() {
    let mut map = MemoryMap::new();
    map.insert(u64::MAX, 1, DeviceId(1)).unwrap();
    let config = MemoryStriperConfig::new(u64::MAX, 1, 64, DeviceId(9), map);
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let model =
        MemoryStriper::new_and_register(&engine, &clock, engine.top(), "striper", config).unwrap();
    let Ports {
        mut input,
        mut output,
        mut children,
        mut replies,
    } = ports(&mut engine, &model);
    let parent = request(&engine, AccessType::ReadRequest, u64::MAX, 1);
    engine.spawn(async move {
        input.put(parent)?.await;
        Ok(())
    });
    engine.spawn(async move {
        let child = children.get()?.await;
        assert_eq!(child.dst_addr(), u64::MAX);
        replies.put(child.to_response(&ReplyMemory)?)?.await;
        Ok(())
    });
    let completed = Rc::new(std::cell::Cell::new(false));
    let observed = completed.clone();
    engine.spawn(async move {
        assert_eq!(output.get()?.await.dst_addr(), u64::MAX);
        observed.set(true);
        Ok(())
    });
    engine.run().unwrap();
    assert!(completed.get());
}
