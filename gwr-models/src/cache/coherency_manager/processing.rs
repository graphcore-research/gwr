// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Clocked directory processing, independent of ingress and output handshakes.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gwr_engine::events::repeated::Repeated;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Event, SimObject};
use gwr_engine::types::{AccessType, SimResult};

use super::{CoherenceOp, CoherencyManager};
use crate::cache::traits::CoherentAccess;

pub(super) async fn run<T: SimObject + CoherentAccess>(
    manager: &CoherencyManager<T>,
    rx: InPort<T>,
    tx: OutPort<T>,
) -> SimResult {
    let inbox = Rc::new(RefCell::new(Inbox::default()));
    let output = Rc::new(RefCell::new(VecDeque::new()));
    let changed = Repeated::default();
    manager
        .spawner
        .spawn(receive_packets(rx, inbox.clone(), changed.clone()));
    manager.spawner.spawn(send_packets(
        tx,
        output.clone(),
        changed.clone(),
        manager.sent.clone(),
    ));
    let limit = manager.state.borrow().config.operations_per_tick;
    let mut budget = ProcessingBudget::new(limit);
    loop {
        let has_work = !inbox.borrow().is_empty() || manager.state.borrow().has_pending_work();
        if !has_work {
            changed.listen().await;
            continue;
        }
        budget.wait(&manager.clock).await?;
        // Select after waiting so newly arrived protocol traffic can make
        // progress ahead of ordinary requests and internal replay work.
        let protocol = inbox.borrow_mut().protocol.pop_front();
        let messages = if let Some(packet) = protocol {
            manager.state.borrow_mut().process_packet(packet)?
        } else if manager.state.borrow().has_pending_work() {
            manager.state.borrow_mut().process_pending()?.unwrap()
        } else {
            let packet = inbox.borrow_mut().requests.pop_front().unwrap();
            manager.state.borrow_mut().process_packet(packet)?
        };
        budget.spend();
        output.borrow_mut().extend(messages);
        changed.notify();
    }
}

async fn receive_packets<T: SimObject + CoherentAccess>(
    mut rx: InPort<T>,
    inbox: Rc<RefCell<Inbox<T>>>,
    changed: Repeated<()>,
) -> SimResult {
    loop {
        let packet = rx.get()?.await;
        inbox.borrow_mut().push(packet);
        changed.notify();
    }
}

async fn send_packets<T: SimObject + CoherentAccess>(
    mut tx: OutPort<T>,
    output: Rc<RefCell<VecDeque<T>>>,
    changed: Repeated<()>,
    sent: Rc<RefCell<[usize; CoherenceOp::COUNT]>>,
) -> SimResult {
    loop {
        let next = output.borrow_mut().pop_front();
        if let Some(packet) = next {
            let op = packet.coherence_op();
            tx.put(packet)?.await;
            if let Some(op) = op {
                sent.borrow_mut()[op as usize] += 1;
            }
        } else {
            changed.listen().await;
        }
    }
}

struct Inbox<T> {
    protocol: VecDeque<T>,
    requests: VecDeque<T>,
}

impl<T> Default for Inbox<T> {
    fn default() -> Self {
        Self {
            protocol: VecDeque::new(),
            requests: VecDeque::new(),
        }
    }
}

impl<T: CoherentAccess> Inbox<T> {
    fn push(&mut self, packet: T) {
        let protocol = matches!(
            packet.coherence_op(),
            Some(CoherenceOp::InvalidateAck | CoherenceOp::Writeback)
        ) || matches!(
            packet.access_type(),
            AccessType::ReadResponse | AccessType::WriteNonPostedResponse
        );
        if protocol {
            self.protocol.push_back(packet);
        } else {
            self.requests.push_back(packet);
        }
    }

    fn is_empty(&self) -> bool {
        self.protocol.is_empty() && self.requests.is_empty()
    }
}

struct ProcessingBudget {
    limit: Option<usize>,
    tick: u64,
    used: usize,
}

impl ProcessingBudget {
    fn new(limit: Option<usize>) -> Self {
        Self {
            limit,
            tick: 0,
            used: 0,
        }
    }

    async fn wait(&mut self, clock: &Clock) -> SimResult {
        let Some(limit) = self.limit else {
            return Ok(());
        };
        let now = clock.tick_now().tick();
        if now != self.tick {
            self.tick = now;
            self.used = 0;
        }
        if self.used == limit {
            clock.wait_ticks(1)?.await;
            self.tick = clock.tick_now().tick();
            self.used = 0;
        }
        Ok(())
    }

    fn spend(&mut self) {
        if self.limit.is_some() {
            self.used += 1;
        }
    }
}
