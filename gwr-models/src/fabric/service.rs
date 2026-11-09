// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Bounded FIFO route service with exact bit debt and independent publication.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Event, Runnable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::Id;
use gwr_track::entity::Entity;
use serde::Serialize;

/// Ownership and transport accounting for one directed route.
#[derive(Clone, Debug, Default, Serialize)]
pub struct RouteServiceSnapshot {
    /// Trace entity identifying the plane and directed route.
    pub route: String,
    /// Accepted transport packets.
    pub admitted_packets: u64,
    /// Packets whose tails have arrived and whose heads were acknowledged
    /// downstream.
    pub completed_packets: u64,
    /// Accepted transport bytes, including headers.
    pub admitted_bytes: u64,
    /// Completed transport bytes after tail arrival and downstream
    /// acknowledgement.
    pub completed_bytes: u64,
    /// Admitted payload bytes when known for every accepted packet.
    pub admitted_payload_bytes: Option<u64>,
    /// Admitted header bytes when known for every accepted packet.
    pub admitted_header_bytes: Option<u64>,
    /// Acknowledged payload bytes when known for every completed packet.
    pub completed_payload_bytes: Option<u64>,
    /// Acknowledged header bytes when known for every completed packet.
    pub completed_header_bytes: Option<u64>,
    /// All packets still owned, including the publishing packet.
    pub owned_packets: usize,
    /// All transport bytes still owned.
    pub owned_bytes: usize,
    /// Peak owned packets.
    pub peak_packets: usize,
    /// Peak owned transport bytes.
    pub peak_bytes: usize,
    /// Cumulative time waiting for downstream ACK after service eligibility.
    pub blocked_ticks: u64,
    /// Cumulative exact bit service, rounded once to ticks.
    pub service_ticks: u64,
}

/// Head and tail availability in exact bit-time units at the fabric's common
/// path rate. Keeping fractions across hops avoids per-hop tick rounding.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ForwardingTiming {
    pub head: u128,
    pub tail: u128,
}

impl ForwardingTiming {
    pub(crate) fn tail_tick(self, rate: usize) -> Result<u64, SimError> {
        service_tick(self.tail, rate)
    }
}

type TimingAccessor<T> = fn(&mut T) -> &mut ForwardingTiming;

fn service_tick(units: u128, rate: usize) -> Result<u64, SimError> {
    u64::try_from(units.div_ceil(rate as u128))
        .map_err(|_| SimError("route service tick overflows".into()))
}

struct Completion {
    tick: u64,
    bytes: usize,
    payload: Option<usize>,
    id: Id,
}

struct Ticket<T> {
    value: T,
    deadline: u64,
    completion_tick: u64,
}
struct State<T> {
    queue: VecDeque<Ticket<T>>,
    completions: VecDeque<Completion>,
    exact_end: u128,
    bits: u128,
    snapshot: RouteServiceSnapshot,
}

/// One independently queued, directed, pipelined service resource.
#[derive(EntityGet, EntityDisplay)]
pub struct RouteService<T: SimObject> {
    entity: Rc<Entity>,
    clock: Clock,
    rate: Option<usize>,
    timing: Option<TimingAccessor<T>>,
    delay: usize,
    byte_capacity: usize,
    packet_capacity: usize,
    state: RefCell<State<T>>,
    changed: Repeated<()>,
    rx: RefCell<Option<InPort<T>>>,
    tx: RefCell<Option<OutPort<T>>>,
}
impl<T: SimObject> RouteService<T> {
    /// Construct a bounded route. Rate is in bits per supplied Clock tick.
    #[expect(clippy::too_many_arguments)]
    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        bw_bits_per_tick: usize,
        delay_ticks: usize,
        queue_bytes: usize,
        queue_packets: usize,
    ) -> Result<Rc<Self>, SimError> {
        Self::build(
            engine,
            clock,
            parent,
            name,
            Some(bw_bits_per_tick),
            delay_ticks,
            queue_bytes,
            queue_packets,
            None,
        )
    }
    /// Bounded storage without serialization; both credits remain owned through
    /// ACK.
    pub fn new_queue_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        queue_bytes: usize,
        queue_packets: usize,
    ) -> Result<Rc<Self>, SimError> {
        Self::build(
            engine,
            clock,
            parent,
            name,
            None,
            0,
            queue_bytes,
            queue_packets,
            None,
        )
    }
    /// Internal streaming routes pass availability metadata with the head;
    /// full-packet transmission credit survives until the tail and ACK.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new_forwarding_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        rate: usize,
        delay: usize,
        queue_bytes: usize,
        queue_packets: usize,
        timing: TimingAccessor<T>,
    ) -> Result<Rc<Self>, SimError> {
        Self::build(
            engine,
            clock,
            parent,
            name,
            Some(rate),
            delay,
            queue_bytes,
            queue_packets,
            Some(timing),
        )
    }

    #[expect(clippy::too_many_arguments)]
    fn build(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        bw_bits_per_tick: Option<usize>,
        delay_ticks: usize,
        queue_bytes: usize,
        queue_packets: usize,
        timing: Option<TimingAccessor<T>>,
    ) -> Result<Rc<Self>, SimError> {
        if bw_bits_per_tick == Some(0) || queue_bytes == 0 || queue_packets == 0 {
            return Err(SimError(
                "route bandwidth and queue bounds must be positive".into(),
            ));
        }
        let entity = Rc::new(Entity::new(parent, name));
        let route = Rc::new(Self {
            rx: RefCell::new(Some(InPort::new(engine, clock, &entity, "rx"))),
            tx: RefCell::new(Some(OutPort::new(&entity, "tx"))),
            entity,
            clock: clock.clone(),
            rate: bw_bits_per_tick,
            timing,
            delay: delay_ticks,
            byte_capacity: queue_bytes,
            packet_capacity: queue_packets,
            state: RefCell::new(State {
                queue: VecDeque::new(),
                completions: VecDeque::new(),
                exact_end: 0,
                bits: 0,
                snapshot: RouteServiceSnapshot {
                    admitted_payload_bytes: Some(0),
                    admitted_header_bytes: Some(0),
                    completed_payload_bytes: Some(0),
                    completed_header_bytes: Some(0),
                    ..Default::default()
                },
            }),
            changed: Repeated::default(),
        });
        engine.register(route.clone());
        Ok(route)
    }
    /// Ingress admission port. ACK means admitted, not delivered.
    pub fn port_rx(&self) -> PortStateResult<T> {
        self.rx
            .borrow()
            .as_ref()
            .expect("route not started")
            .state()
    }
    /// Connect the downstream recipient.
    pub fn connect_port_tx(&self, port: PortStateResult<T>) -> SimResult {
        self.tx
            .borrow_mut()
            .as_mut()
            .expect("route not started")
            .connect(port)
    }
    /// Observe actual admitted, retained, and completed work.
    pub fn snapshot(&self) -> RouteServiceSnapshot {
        let mut snapshot = self.state.borrow().snapshot.clone();
        snapshot.route = self.entity.to_string();
        snapshot
    }
    /// Admit one packet, waiting for both byte and packet ownership capacity.
    #[expect(
        clippy::too_many_lines,
        reason = "Keep packet admission and forwarding deadline accounting in one transaction"
    )]
    pub(crate) async fn enqueue(&self, mut value: T) -> SimResult {
        let bytes = value.total_bytes();
        let payload = value.payload_bytes().filter(|p| *p <= bytes);
        if bytes > self.byte_capacity {
            return Err(SimError(format!(
                "route packet of {bytes} bytes exceeds queue capacity {}",
                self.byte_capacity
            )));
        }
        loop {
            {
                let state = self.state.borrow();
                if state.snapshot.owned_packets < self.packet_capacity
                    && bytes <= self.byte_capacity - state.snapshot.owned_bytes
                {
                    break;
                }
            }
            self.changed.listen().await;
        }
        let mut state = self.state.borrow_mut();
        let bits = (bytes as u128)
            .checked_mul(8)
            .ok_or_else(|| SimError("route packet bits overflow".into()))?;
        let total_bits = state
            .bits
            .checked_add(bits)
            .ok_or_else(|| SimError("route bit accounting overflows".into()))?;
        let now = self.clock.tick_now().tick();
        let (exact_end, deadline, completion_tick, ticks) = if let Some(rate) = self.rate {
            let rate_units = rate as u128;
            let delay = (self.delay as u128) * rate_units;
            let (head, tail) = if let Some(access) = self.timing {
                let incoming = *access(&mut value);
                // A delayed admission can receive a partially or fully buffered
                // packet. Preserve fractional arrival time when only simulator
                // tick rounding delayed the event.
                let head = if now > service_tick(incoming.head, rate)? {
                    u128::from(now) * rate_units
                } else {
                    incoming.head
                };
                (head, incoming.tail.max(head))
            } else {
                let now = u128::from(now) * rate_units;
                (now, now)
            };
            let start = state.exact_end.max(head);
            let end = start
                .checked_add(bits)
                .ok_or_else(|| SimError("route service deadline overflows".into()))?
                .max(tail);
            let output_tail = end
                .checked_add(delay)
                .ok_or_else(|| SimError("route tail deadline overflows".into()))?;
            let output_head = if let Some(access) = self.timing {
                let output_head = start
                    .checked_add(delay)
                    .ok_or_else(|| SimError("route head deadline overflows".into()))?;
                *access(&mut value) = ForwardingTiming {
                    head: output_head,
                    tail: output_tail,
                };
                output_head
            } else {
                output_tail
            };
            (
                end,
                service_tick(output_head, rate)?,
                service_tick(output_tail, rate)?,
                service_tick(total_bits, rate)?,
            )
        } else {
            (0, now, now, 0)
        };
        let packets = state
            .snapshot
            .admitted_packets
            .checked_add(1)
            .ok_or_else(|| SimError("route packet counter overflows".into()))?;
        let admitted_bytes = state
            .snapshot
            .admitted_bytes
            .checked_add(u64::try_from(bytes).map_err(|_| SimError("route size overflows".into()))?)
            .ok_or_else(|| SimError("route byte counter overflows".into()))?;
        state.queue.push_back(Ticket {
            value,
            deadline,
            completion_tick,
        });
        state.exact_end = exact_end;
        state.bits = total_bits;
        let s = &mut state.snapshot;
        s.admitted_packets = packets;
        s.admitted_bytes = admitted_bytes;
        s.admitted_payload_bytes = s
            .admitted_payload_bytes
            .zip(payload)
            .and_then(|(total, n)| total.checked_add(n as u64));
        s.admitted_header_bytes = s
            .admitted_header_bytes
            .zip(payload)
            .and_then(|(total, n)| total.checked_add((bytes - n) as u64));
        s.owned_packets += 1;
        s.owned_bytes += bytes;
        s.peak_packets = s.peak_packets.max(s.owned_packets);
        s.peak_bytes = s.peak_bytes.max(s.owned_bytes);
        s.service_ticks = ticks;
        drop(state);
        self.changed.notify();
        Ok(())
    }
}
#[async_trait(?Send)]
impl<T: SimObject> Runnable for RouteService<T> {
    async fn run(&self) -> SimResult {
        let mut rx = self.rx.borrow_mut().take().unwrap();
        let mut tx = self.tx.borrow_mut().take().unwrap();
        // The engine owns this service for its entire run; the receiver and
        // publisher are separate futures so a stalled output retains bounded
        // completed ownership without blocking independent admission.
        let intake = async {
            loop {
                let value = rx.start_get()?.await;
                self.enqueue(value).await?;
                rx.finish_get();
            }
            #[allow(unreachable_code)]
            Ok::<(), SimError>(())
        };
        let publish = async {
            loop {
                let ticket = loop {
                    let next = self.state.borrow_mut().queue.pop_front();
                    if let Some(ticket) = next {
                        break ticket;
                    }
                    self.changed.listen().await;
                };
                self.clock
                    .wait_ticks(ticket.deadline.saturating_sub(self.clock.tick_now().tick()))?
                    .await;
                let bytes = ticket.value.total_bytes();
                let payload = ticket.value.payload_bytes().filter(|p| *p <= bytes);
                let before = self.clock.tick_now().tick();
                self.entity.track_enter(ticket.value.id());
                let id = ticket.value.id();
                tx.put(ticket.value)?.await;
                let now = self.clock.tick_now().tick();
                let mut state = self.state.borrow_mut();
                state.snapshot.blocked_ticks = state
                    .snapshot
                    .blocked_ticks
                    .checked_add(now - before)
                    .ok_or_else(|| SimError("route blocked counter overflows".into()))?;
                state.completions.push_back(Completion {
                    tick: ticket.completion_tick.max(now),
                    bytes,
                    payload,
                    id,
                });
                drop(state);
                self.changed.notify();
            }
            #[allow(unreachable_code)]
            Ok::<(), SimError>(())
        };
        // Forwarding and credit retirement are independent: the next head can
        // advance while a prior tail is still traversing the link latency.
        let retire = async {
            loop {
                let completion = loop {
                    if let Some(completion) = self.state.borrow_mut().completions.pop_front() {
                        break completion;
                    }
                    self.changed.listen().await;
                };
                self.clock
                    .wait_ticks(completion.tick.saturating_sub(self.clock.tick_now().tick()))?
                    .await;
                self.entity.track_exit(completion.id);
                let bytes = completion.bytes;
                let payload = completion.payload;
                let mut state = self.state.borrow_mut();
                let s = &mut state.snapshot;
                s.completed_packets += 1;
                s.completed_bytes += bytes as u64;
                s.completed_payload_bytes = s
                    .completed_payload_bytes
                    .zip(payload)
                    .and_then(|(total, n)| total.checked_add(n as u64));
                s.completed_header_bytes = s
                    .completed_header_bytes
                    .zip(payload)
                    .and_then(|(total, n)| total.checked_add((bytes - n) as u64));
                s.owned_packets -= 1;
                s.owned_bytes -= bytes;
                drop(state);
                self.changed.notify();
            }
            #[allow(unreachable_code)]
            Ok::<(), SimError>(())
        };
        futures::try_join!(intake, publish, retire)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use gwr_engine::traits::TotalBytes;
    use gwr_engine::types::{AccessType, DeviceId};
    use gwr_track::id::Unique;

    use super::*;
    use crate::memory::memory_access::MemoryAccess;

    #[derive(Clone, Debug)]
    struct Packet {
        payload: MemoryAccess,
        timing: ForwardingTiming,
    }
    impl Unique for Packet {
        fn id(&self) -> Id {
            self.payload.id()
        }
    }
    impl TotalBytes for Packet {
        fn total_bytes(&self) -> usize {
            self.payload.total_bytes()
        }
    }
    impl fmt::Display for Packet {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            fmt::Display::fmt(&self.payload, f)
        }
    }
    impl SimObject for Packet {}

    #[test]
    fn delayed_head_admission_preserves_tail_availability() {
        for (incoming_tail, expected_tail) in [(0, 24), (100, 102)] {
            let mut engine = Engine::default();
            let clock = engine.default_clock();
            let service = RouteService::new_forwarding_and_register(
                &engine,
                &clock,
                engine.top(),
                "route",
                256,
                2,
                64,
                1,
                |p: &mut Packet| &mut p.timing,
            )
            .unwrap();
            let mut source = OutPort::new(engine.top(), "source");
            source.connect(service.port_rx()).unwrap();
            let mut sink = InPort::new(&engine, &clock, engine.top(), "sink");
            service.connect_port_tx(sink.state()).unwrap();
            let packets = [0, incoming_tail].map(|tail| Packet {
                payload: MemoryAccess::new(
                    engine.top(),
                    AccessType::ReadResponse,
                    64,
                    0,
                    0,
                    DeviceId(0),
                    DeviceId(1),
                    0,
                ),
                timing: ForwardingTiming {
                    head: 0,
                    tail: tail * 256,
                },
            });
            engine.spawn(async move {
                for packet in packets {
                    source.put(packet)?.await;
                }
                Ok(())
            });
            let c = clock.clone();
            engine.spawn(async move {
                c.wait_ticks(20)?.await;
                let first = sink.get()?.await;
                assert_eq!(c.tick_now().tick(), 20);
                assert_eq!(first.timing.tail_tick(256)?, 4);
                let second = sink.get()?.await;
                assert_eq!(c.tick_now().tick(), 22);
                assert_eq!(second.timing.tail_tick(256)?, expected_tail);
                Ok(())
            });
            let c = clock.clone();
            let s = service.clone();
            engine.spawn(async move {
                c.wait_ticks(21)?.await;
                let snapshot = s.snapshot();
                assert_eq!(snapshot.completed_packets, 1);
                assert_eq!(snapshot.owned_packets, 1);
                Ok(())
            });
            engine.run().unwrap();
            assert_eq!(clock.tick_now().tick(), expected_tail);
            let snapshot = service.snapshot();
            assert_eq!(snapshot.completed_packets, 2);
            assert_eq!(snapshot.owned_packets, 0);
            assert_eq!(snapshot.owned_bytes, 0);
            assert_eq!(snapshot.peak_packets, 1);
            assert_eq!(snapshot.peak_bytes, 64);
        }
    }
}
