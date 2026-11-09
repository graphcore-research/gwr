// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Actual ACKed signal/fetch values, not a fixed winning source partition.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gwr_engine::events::repeated::Repeated;
use gwr_engine::traits::Event;
use gwr_engine::types::{SimError, SimResult};

use super::{
    AtomicTargetSender, Completion, ControlEffect, ControlGroup, Effect, SharedAtomicControl,
};
fn error(s: &str) -> SimError {
    SimError(s.into())
}
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParticipantObservation {
    pub polls: u64,
    pub zero_polls: u64,
    pub fetched: Option<u32>,
    pub output_acks: BTreeSet<u64>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerObservation {
    pub participants: BTreeMap<u32, ParticipantObservation>,
    pub last: Option<u32>,
    pub reset_semaphore: bool,
    pub reset_signal: bool,
    pub failed: bool,
    pub reply_ids: BTreeSet<u64>,
}
struct State {
    observation: ControllerObservation,
    poll_started: BTreeSet<u32>,
    fetch_started: BTreeSet<u32>,
    outputs_started: BTreeSet<(u32, u64)>,
}
pub struct SplitKController {
    group: ControlGroup,
    target: Rc<AtomicTargetSender>,
    control: SharedAtomicControl,
    state: RefCell<State>,
    changed: Repeated<()>,
    #[cfg(test)]
    before_fetch_ack: RefCell<
        Option<(
            futures::channel::oneshot::Sender<Completion>,
            futures::channel::oneshot::Receiver<()>,
        )>,
    >,
}
impl std::fmt::Debug for SplitKController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SplitKController")
            .field("group", &self.group)
            .field("observation", &self.observation())
            .finish()
    }
}
// A dropped/canceled actual execution future cannot leave usable local
// continuation authority. Already committed physical effects stay in service.
struct Execution<'a> {
    controller: &'a SplitKController,
    successful: bool,
}
impl Drop for Execution<'_> {
    fn drop(&mut self) {
        if !self.successful {
            self.controller.state.borrow_mut().observation.failed = true;
            self.controller.changed.notify();
        }
    }
}
impl SplitKController {
    /// Called once when binding the independently admitted complete group.
    /// Repeated role preflight below never registers or issues a request.
    pub fn new(
        group: ControlGroup,
        target: Rc<AtomicTargetSender>,
        control: SharedAtomicControl,
    ) -> Result<Rc<Self>, SimError> {
        if group.device != target.device() || !control.matches_target(&target) {
            return Err(error("controller target device mismatch"));
        }
        control.register_control(group.clone())?;
        let participants = (0..group.participants)
            .map(|p| (p, ParticipantObservation::default()))
            .collect();
        Ok(Rc::new(Self {
            group,
            target,
            control,
            state: RefCell::new(State {
                observation: ControllerObservation {
                    participants,
                    ..Default::default()
                },
                poll_started: BTreeSet::new(),
                fetch_started: BTreeSet::new(),
                outputs_started: BTreeSet::new(),
            }),
            changed: Repeated::default(),
            #[cfg(test)]
            before_fetch_ack: RefCell::new(None),
        }))
    }
    pub fn targets(&self, target: &Rc<AtomicTargetSender>) -> bool {
        Rc::ptr_eq(&self.target, target)
    }
    pub fn group(&self) -> &ControlGroup {
        &self.group
    }
    pub fn observation(&self) -> ControllerObservation {
        self.state.borrow().observation.clone()
    }
    pub fn validate_role(&self, address: u64, effect: Effect) -> SimResult {
        let g = &self.group;
        let (invocation, generation) = match effect {
            Effect::Atomic { word, .. } => (word.invocation, word.generation),
            Effect::Control {
                invocation,
                generation,
                ..
            }
            | Effect::InitializeOutput {
                invocation,
                generation,
            } => (invocation, generation),
            _ => return Err(error("ordinary effect cannot use SplitK controller")),
        };
        if (invocation, generation) != (g.invocation, g.generation) {
            return Err(error("controller exact invocation/generation mismatch"));
        }
        if let Effect::Control {
            participant: u32::MAX,
            effect: ControlEffect::ResetSemaphore | ControlEffect::ResetSignal,
            ..
        } = effect
        {
            let Effect::Control { effect, .. } = effect else {
                unreachable!()
            };
            let expected = if effect == ControlEffect::ResetSemaphore {
                g.semaphore
            } else {
                g.signal
            };
            if address != expected {
                return Err(error("dynamic reset address mismatch"));
            }
            return Ok(());
        }
        self.target.validate_effect(g.device, address, effect)
    }
    fn live(&self) -> SimResult {
        if self.state.borrow().observation.failed {
            Err(error("SplitK controller canceled/failed; no continuation"))
        } else {
            Ok(())
        }
    }
    fn record(&self, completion: Completion) -> SimResult {
        if !self
            .state
            .borrow_mut()
            .observation
            .reply_ids
            .insert(completion.identity)
        {
            return Err(error("controller duplicate actual reply identity"));
        }
        Ok(())
    }
    async fn issue(&self, address: u64, effect: Effect) -> Result<Completion, SimError> {
        self.issue_with_operand(address, effect, None).await
    }
    async fn issue_with_operand(
        &self,
        address: u64,
        effect: Effect,
        operand: Option<Rc<crate::processing_element::LocalOperandHandle>>,
    ) -> Result<Completion, SimError> {
        self.live()?;
        let receipt = self
            .target
            .receive_with_operand(self.group.device, address, effect, operand)
            .await?;
        #[cfg(test)]
        if matches!(
            effect,
            Effect::Control {
                effect: ControlEffect::FetchAddOne,
                ..
            }
        ) {
            let pause = self.before_fetch_ack.borrow_mut().take();
            if let Some((seen, resume)) = pause {
                let _ = seen.send(receipt.completion());
                resume
                    .await
                    .map_err(|_| error("test actual fetch ACK pause abandoned"))?;
            }
        }
        let actual = receipt.acknowledge().await?;
        // Drain the real ACK first, then reject sibling cancellation before
        // recording any received value or successful continuation authority.
        self.live()?;
        self.record(actual)?;
        Ok(actual)
    }
    pub async fn execute(&self, address: u64, effect: Effect) -> Result<Completion, SimError> {
        self.execute_with_operand(address, effect, None).await
    }
    #[expect(
        clippy::too_many_lines,
        reason = "Keep the ordered fetch reset and publication protocol visible together"
    )]
    pub(crate) async fn execute_with_operand(
        &self,
        address: u64,
        effect: Effect,
        operand: Option<Rc<crate::processing_element::LocalOperandHandle>>,
    ) -> Result<Completion, SimError> {
        if operand.is_some() && !matches!(effect, Effect::Atomic { .. }) {
            return Err(error(
                "scratch operand cannot authorize control continuation",
            ));
        }
        self.validate_role(address, effect)?;
        self.live()?;
        let mut guard = Execution {
            controller: self,
            successful: false,
        };
        let g = &self.group;
        let result = match effect {
            Effect::Control {
                participant,
                effect: ControlEffect::PollSignal,
                ..
            } => {
                if !self.state.borrow_mut().poll_started.insert(participant) {
                    return Err(error("participant poll role already started"));
                }
                loop {
                    let reply = self.issue(address, effect).await?;
                    let value = reply
                        .control_value
                        .ok_or_else(|| error("actual poll reply has no signal value"))?;
                    if value > 1 {
                        return Err(error("source signal reply is outside zero/one"));
                    }
                    {
                        let mut state = self.state.borrow_mut();
                        let p = state
                            .observation
                            .participants
                            .get_mut(&participant)
                            .unwrap();
                        p.polls = p
                            .polls
                            .checked_add(1)
                            .ok_or_else(|| error("poll count overflow"))?;
                        if value == 0 {
                            p.zero_polls = p
                                .zero_polls
                                .checked_add(1)
                                .ok_or_else(|| error("zero-poll count overflow"))?;
                        }
                    }
                    if value == 1 {
                        break reply;
                    }
                    // Every iteration charged actual 4B read and positive
                    // shared byte/lane debt. No guessed sleep or free polling.
                }
            }
            Effect::Control {
                participant,
                effect: ControlEffect::FetchAddOne,
                ..
            } => {
                if !self.state.borrow_mut().fetch_started.insert(participant) {
                    return Err(error("participant fetch role already started"));
                }
                let reply = self.issue(address, effect).await?;
                let old = reply
                    .control_value
                    .ok_or_else(|| error("actual fetch reply lacks old count"))?;
                if old >= g.participants {
                    return Err(error("actual old semaphore outside participant roster"));
                }
                let mut state = self.state.borrow_mut();
                state
                    .observation
                    .participants
                    .get_mut(&participant)
                    .unwrap()
                    .fetched = Some(old);
                if old == g.participants - 1
                    && state.observation.last.replace(participant).is_some()
                {
                    return Err(error("multiple actual last participants"));
                }
                drop(state);
                self.changed.notify();
                reply
            }
            Effect::Control {
                participant: u32::MAX,
                effect: reset @ (ControlEffect::ResetSemaphore | ControlEffect::ResetSignal),
                ..
            } => {
                let last = {
                    let state = self.state.borrow();
                    let last = state
                        .observation
                        .last
                        .ok_or_else(|| error("dynamic reset lacks actual last fetch receipt"))?;
                    if state
                        .observation
                        .participants
                        .values()
                        .any(|p| p.fetched.is_none())
                    {
                        return Err(error("reset must follow every actual fetch ACK"));
                    }
                    if reset == ControlEffect::ResetSignal && !state.observation.reset_semaphore {
                        return Err(error("signal reset must follow semaphore reset ACK"));
                    }
                    last
                };
                let actual = Effect::Control {
                    invocation: g.invocation,
                    generation: g.generation,
                    participant: last,
                    effect: reset,
                };
                let reply = self.issue(address, actual).await?;
                let mut state = self.state.borrow_mut();
                if reset == ControlEffect::ResetSemaphore {
                    state.observation.reset_semaphore = true;
                } else {
                    state.observation.reset_signal = true;
                }
                drop(state);
                self.changed.notify();
                reply
            }
            Effect::Atomic { participant, word } => {
                if !self
                    .state
                    .borrow_mut()
                    .outputs_started
                    .insert((participant, word.address))
                {
                    return Err(error("participant output word already started"));
                }
                if self.state.borrow().observation.participants[&participant]
                    .fetched
                    .is_none()
                {
                    return Err(error("output requires actual participant fetch ACK"));
                }
                loop {
                    let changed = self.changed.listen();
                    self.live()?;
                    let waits = {
                        let state = self.state.borrow();
                        state.observation.last == Some(participant)
                            && !state.observation.reset_signal
                    };
                    if !waits {
                        break;
                    }
                    changed.await;
                }
                let reply = self.issue_with_operand(address, effect, operand).await?;
                self.state
                    .borrow_mut()
                    .observation
                    .participants
                    .get_mut(&participant)
                    .unwrap()
                    .output_acks
                    .insert(address);
                self.changed.notify();
                reply
            }
            _ => self.issue(address, effect).await?,
        };
        guard.successful = true;
        Ok(result)
    }
    pub fn verify_final_join(&self) -> SimResult {
        self.live()?;
        let state = self.state.borrow();
        if !state.observation.reset_signal
            || state
                .observation
                .participants
                .values()
                .any(|p| p.output_acks.len() != self.group.output_words.len())
        {
            return Err(error(
                "final join requires every output actual ACK, not semaphore zero",
            ));
        }
        if !self
            .control
            .control_finished(self.group.invocation, self.group.generation)?
        {
            return Err(error("actual service group is not finished"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use gwr_engine::engine::Engine;
    use gwr_engine::port::{InPort, OutPort};

    use super::*;
    use crate::memory::atomic::*;
    use crate::memory::memory_access::MemoryAccess;
    use crate::memory::{Memory, MemoryConfig};
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep the ordered fetch reset and publication protocol visible together"
    )]
    fn actual_granted_fetch_cancel_drains_real_ack_without_output_reset_or_reusable_generation() {
        for cancel in [false, true] {
            let mut engine = Engine::default();
            let clock = engine.default_clock();
            let cfg = AtomicServiceConfig {
                capacity: 2,
                bytes_per_tick: 16,
                delay_ticks: 0,
                bf16_pair_per_tick: Some(1),
                i32_fetch_add_per_tick: Some(1),
                lane_interleave_bytes: 4,
                lane_bytes_per_tick: vec![16],
            };
            let memory = Memory::<MemoryAccess>::new_and_register(
                &engine,
                &clock,
                engine.top(),
                "m",
                MemoryConfig::new(4096, 256, 16, 0)
                    .with_arrival_aware_service(2)
                    .with_service_lanes(4, vec![16])
                    .with_atomic_service(7, cfg),
            )
            .unwrap();
            let mut idle = OutPort::new(engine.top(), "ordinary_idle");
            idle.connect(memory.port_rx()).unwrap();
            let sink = InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "ordinary_sink");
            memory.connect_port_tx(sink.state()).unwrap();
            let group = ControlGroup {
                invocation: 9,
                generation: 1,
                device: 7,
                signal: 4224,
                semaphore: 4228,
                participants: 1,
                publisher: 0,
                output_words: vec![4096],
                source_flags: SourceFlags::SplitKHgemmSc0Sc1,
            };
            let authority = memory.atomic_control().unwrap();
            let controller = SplitKController::new(
                group.clone(),
                memory.atomic_target().unwrap(),
                authority.clone(),
            )
            .unwrap();
            let (seen, received) = futures::channel::oneshot::channel();
            let (resume, go) = futures::channel::oneshot::channel();
            *controller.before_fetch_ack.borrow_mut() = Some((seen, go));
            let driver = controller.clone();
            let ctl = authority.clone();
            let c = clock.clone();
            engine.spawner().spawn(async move {
                let completion = received
                    .await
                    .map_err(|_| error("actual granted fetch never reached"))?;
                assert_eq!(
                    (completion.identity, completion.control_value),
                    (4, Some(0))
                );
                let before = ctl.snapshot();
                assert_eq!(
                    (
                        before.current,
                        before.completed,
                        before.i32_fetch_add_requests
                    ),
                    (1, 3, 1)
                );
                c.wait_ticks(2)?.await;
                assert_eq!(ctl.snapshot(), before);
                if cancel {
                    ctl.cancel_owner(4)?;
                    assert_eq!(ctl.snapshot().canceled_aftercommit_outstanding, 1);
                }
                assert_eq!(driver.observation().participants[&0].fetched, None);
                resume
                    .send(())
                    .map_err(|_| error("actual fetch receiver abandoned"))?;
                Ok(())
            });
            let task = controller.clone();
            let ctl = authority.clone();
            engine.spawner().spawn(async move {
                let role = |participant, effect| Effect::Control {
                    invocation: 9,
                    generation: 1,
                    participant,
                    effect,
                };
                task.execute(
                    4096,
                    Effect::InitializeOutput {
                        invocation: 9,
                        generation: 1,
                    },
                )
                .await?;
                task.execute(4224, role(0, ControlEffect::PublishSignalOne))
                    .await?;
                task.execute(4224, role(0, ControlEffect::PollSignal))
                    .await?;
                let fetched = task
                    .execute(4228, role(0, ControlEffect::FetchAddOne))
                    .await;
                if cancel {
                    assert!(fetched.unwrap_err().to_string().contains("canceled"));
                    let after = ctl.snapshot();
                    assert_eq!(
                        (
                            after.current,
                            after.completed,
                            after.canceled_aftercommit,
                            after.committed_write_bytes
                        ),
                        (0, 3, 1, 12)
                    );
                    assert!(
                        task.execute(4228, role(u32::MAX, ControlEffect::ResetSemaphore))
                            .await
                            .is_err()
                    );
                    let pair = Effect::Atomic {
                        participant: 0,
                        word: AtomicWord {
                            device: 7,
                            address: 4096,
                            invocation: 9,
                            generation: 1,
                            operation: AtomicOperation::Bf16PairFadd,
                            scope: AtomicScope::Agent,
                            ordering: AtomicOrdering::Monotonic,
                        },
                    };
                    assert!(task.execute(4096, pair).await.is_err());
                    assert_eq!(ctl.snapshot(), after);
                    let mut next = group.clone();
                    next.generation = 2;
                    assert!(ctl.register_control(next).is_err());
                    assert!(task.verify_final_join().is_err());
                } else {
                    assert_eq!(fetched?.control_value, Some(0));
                    task.execute(4228, role(u32::MAX, ControlEffect::ResetSemaphore))
                        .await?;
                    task.execute(4224, role(u32::MAX, ControlEffect::ResetSignal))
                        .await?;
                    task.execute(
                        4096,
                        Effect::Atomic {
                            participant: 0,
                            word: AtomicWord {
                                device: 7,
                                address: 4096,
                                invocation: 9,
                                generation: 1,
                                operation: AtomicOperation::Bf16PairFadd,
                                scope: AtomicScope::Agent,
                                ordering: AtomicOrdering::Monotonic,
                            },
                        },
                    )
                    .await?;
                    task.verify_final_join()?;
                }
                Ok(())
            });
            engine.run().unwrap();
            let observed = controller.observation();
            assert_eq!(observed.failed, cancel);
            if cancel {
                assert_eq!(observed.last, None);
                assert!(!observed.reset_semaphore && !observed.reset_signal);
                assert!(observed.participants[&0].output_acks.is_empty());
            }
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep the ordered fetch reset and publication protocol visible together"
    )]
    fn actual_sibling_drop_during_fetch_ack_drains_without_continuation() {
        for abandon in [false, true] {
            let mut engine = Engine::default();
            let clock = engine.default_clock();
            let cfg = AtomicServiceConfig {
                capacity: 2,
                bytes_per_tick: 16,
                delay_ticks: 0,
                bf16_pair_per_tick: Some(1),
                i32_fetch_add_per_tick: Some(1),
                lane_interleave_bytes: 4,
                lane_bytes_per_tick: vec![16],
            };
            let memory = Memory::<MemoryAccess>::new_and_register(
                &engine,
                &clock,
                engine.top(),
                "m",
                MemoryConfig::new(4096, 256, 16, 0)
                    .with_arrival_aware_service(2)
                    .with_service_lanes(4, vec![16])
                    .with_atomic_service(7, cfg),
            )
            .unwrap();
            let mut idle = OutPort::new(engine.top(), "ordinary_idle");
            idle.connect(memory.port_rx()).unwrap();
            let sink = InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "ordinary_sink");
            memory.connect_port_tx(sink.state()).unwrap();
            let group = ControlGroup {
                invocation: 9,
                generation: 1,
                device: 7,
                signal: 4224,
                semaphore: 4228,
                participants: 2,
                publisher: 0,
                output_words: vec![4096],
                source_flags: SourceFlags::SplitKHgemmSc0Sc1,
            };
            let authority = memory.atomic_control().unwrap();
            let controller =
                SplitKController::new(group, memory.atomic_target().unwrap(), authority.clone())
                    .unwrap();
            let (seen, received) = futures::channel::oneshot::channel();
            let (resume, go) = futures::channel::oneshot::channel();
            *controller.before_fetch_ack.borrow_mut() = Some((seen, go));
            let driver = controller.clone();
            let c = clock.clone();
            engine.spawner().spawn(async move {
                let actual = received
                    .await
                    .map_err(|_| error("fetch receipt not reached"))?;
                assert_eq!((actual.identity, actual.control_value), (4, Some(0)));
                assert_eq!(driver.observation().participants[&0].fetched, None);
                if abandon {
                    // Poll an actual sibling execution into its asynchronous
                    // receive, then drop that future while
                    // participant zero owns a real unACKed receipt.
                    let mut sibling = Box::pin(driver.execute(
                        4224,
                        Effect::Control {
                            invocation: 9,
                            generation: 1,
                            participant: 1,
                            effect: ControlEffect::PollSignal,
                        },
                    ));
                    assert!(futures::poll!(sibling.as_mut()).is_pending());
                    drop(sibling);
                    assert!(driver.observation().failed);
                }
                c.wait_ticks(2)?.await;
                resume
                    .send(())
                    .map_err(|_| error("fetch receiver abandoned"))?;
                Ok(())
            });
            let task = controller.clone();
            engine.spawner().spawn(async move {
                let role = |effect| Effect::Control {
                    invocation: 9,
                    generation: 1,
                    participant: 0,
                    effect,
                };
                task.execute(
                    4096,
                    Effect::InitializeOutput {
                        invocation: 9,
                        generation: 1,
                    },
                )
                .await?;
                task.execute(4224, role(ControlEffect::PublishSignalOne))
                    .await?;
                task.execute(4224, role(ControlEffect::PollSignal)).await?;
                let fetched = task.execute(4228, role(ControlEffect::FetchAddOne)).await;
                if abandon {
                    assert!(fetched.unwrap_err().to_string().contains("no continuation"));
                    assert_eq!(task.observation().participants[&0].fetched, None);
                    assert!(!task.observation().reply_ids.contains(&4));
                    assert!(task.verify_final_join().is_err());
                } else {
                    assert_eq!(fetched?.control_value, Some(0));
                    assert_eq!(task.observation().participants[&0].fetched, Some(0));
                    assert!(task.observation().reply_ids.contains(&4));
                }
                Ok(())
            });
            engine.run().unwrap();
            let state = controller.observation();
            assert_eq!(state.failed, abandon);
            assert_eq!(state.last, None);
            assert!(!state.reset_semaphore && !state.reset_signal);
            assert!(
                state
                    .participants
                    .values()
                    .all(|p| p.output_acks.is_empty())
            );
            let physical = authority.snapshot();
            assert_eq!(physical.current, 0);
            assert_eq!(physical.i32_fetch_add_requests, 1);
            assert!(physical.committed_write_bytes >= 12); // irreversible fetch retained in both branches
        }
    }
}
