// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Multi-engine synchronization and communication primitives.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::executor::{ExternalWaitGuard, ExternalWaitHandle};

#[derive(Clone)]
pub struct MultiEngineSync {
    shared: Arc<MultiEngineSyncShared>,
}

#[derive(Clone)]
pub struct MultiEngineSyncParticipant {
    sync: MultiEngineSync,
    participant: usize,
}

struct MultiEngineSyncShared {
    participants: usize,
    state: Mutex<MultiEngineSyncState>,
}

struct MultiEngineSyncState {
    arrivals: Vec<Option<f64>>,
    arrived_count: usize,
    generation: usize,
    completed: VecDeque<CompletedSync>,
    wakers: Vec<Option<Waker>>,
}

struct CompletedSync {
    generation: usize,
    time_ns: f64,
    remaining: usize,
}

impl MultiEngineSyncState {
    fn new(participants: usize) -> Self {
        Self {
            arrivals: vec![None; participants],
            arrived_count: 0,
            generation: 0,
            completed: VecDeque::new(),
            wakers: vec![None; participants],
        }
    }

    fn complete_generation(&mut self) -> (f64, Vec<Waker>) {
        let sync_time_ns = self.arrivals.iter().flatten().copied().fold(0.0, f64::max);
        let remaining = self.arrived_count.saturating_sub(1);
        if remaining > 0 {
            self.completed.push_back(CompletedSync {
                generation: self.generation,
                time_ns: sync_time_ns,
                remaining,
            });
        }
        self.arrivals.fill(None);
        self.arrived_count = 0;
        self.generation += 1;
        let wakers = self.wakers.iter_mut().filter_map(Option::take).collect();

        (sync_time_ns, wakers)
    }

    fn take_completed_time(&mut self, generation: usize) -> Option<f64> {
        let index = self
            .completed
            .iter()
            .position(|sync| sync.generation == generation)?;
        let time_ns = self.completed[index].time_ns;
        self.completed[index].remaining -= 1;
        if self.completed[index].remaining == 0 {
            self.completed.remove(index);
        }
        Some(time_ns)
    }
}

/// Future returned by [`MultiEngineSyncParticipant::sync`].
pub struct MultiEngineSyncFuture {
    sync: MultiEngineSync,
    participant: usize,
    time_ns: f64,
    wait: Option<ExternalWaitGuard>,
    generation: Option<usize>,
    wait_handle: ExternalWaitHandle,
}

impl Future for MultiEngineSyncFuture {
    type Output = f64;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        let mut state = this.sync.shared.state.lock().unwrap();

        if let Some(my_generation) = this.generation {
            if state.generation != my_generation {
                let sync_time_ns = state
                    .take_completed_time(my_generation)
                    .expect("completed sync generation missing");
                this.generation = None;
                drop(state);
                drop(this.wait.take());
                return Poll::Ready(sync_time_ns);
            }

            if this.wait.is_none() {
                this.wait = Some(this.wait_handle.begin_time_blocking_wait());
            }
            state.wakers[this.participant] = Some(cx.waker().clone());
            return Poll::Pending;
        }

        assert!(
            state.arrivals[this.participant].is_none(),
            "sync participant already arrived"
        );
        this.generation = Some(state.generation);
        state.arrivals[this.participant] = Some(this.time_ns);
        state.arrived_count += 1;

        if state.arrived_count == this.sync.shared.participants {
            let (sync_time_ns, wakers) = state.complete_generation();
            drop(state);

            drop(this.wait.take());
            this.generation = None;
            for waker in wakers {
                waker.wake();
            }
            Poll::Ready(sync_time_ns)
        } else {
            if this.wait.is_none() {
                this.wait = Some(this.wait_handle.begin_time_blocking_wait());
            }

            state.wakers[this.participant] = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl Drop for MultiEngineSyncFuture {
    fn drop(&mut self) {
        let Some(generation) = self.generation else {
            return;
        };

        let Ok(mut state) = self.sync.shared.state.lock() else {
            return;
        };
        if state.generation == generation {
            state.wakers[self.participant] = None;
            if state.arrivals[self.participant].take().is_some() {
                state.arrived_count -= 1;
            }
        } else {
            state.take_completed_time(generation);
        }
    }
}

/// MultiEngineSync provides a synchronization primitive for multiple engines to
/// synchronize their execution and share the maximum time_ns among them.
impl MultiEngineSync {
    /// Create a new MultiEngineSync with the specified number of participants.
    /// Panics if participants is zero.
    #[must_use]
    pub fn new(participants: usize) -> Self {
        assert!(participants > 0, "sync must have at least one participant");

        Self {
            shared: Arc::new(MultiEngineSyncShared {
                participants,
                state: Mutex::new(MultiEngineSyncState::new(participants)),
            }),
        }
    }

    #[must_use]
    pub fn participant(&self, participant: usize) -> MultiEngineSyncParticipant {
        assert!(
            participant < self.shared.participants,
            "sync participant out of range"
        );

        MultiEngineSyncParticipant {
            sync: self.clone(),
            participant,
        }
    }
}

impl MultiEngineSyncParticipant {
    /// Synchronize asynchronously and return the maximum participant time.
    #[must_use]
    pub fn sync(&self, time_ns: f64, wait_handle: ExternalWaitHandle) -> MultiEngineSyncFuture {
        MultiEngineSyncFuture {
            sync: self.sync.clone(),
            participant: self.participant,
            time_ns,
            wait: None,
            generation: None,
            wait_handle,
        }
    }
}

pub struct MultiEngineChannel;

pub struct MultiEngineChannelSender<T: Send + 'static> {
    shared: Arc<Mutex<MultiEngineChannelState<T>>>,
}

pub struct MultiEngineChannelReceiver<T: Send + 'static> {
    shared: Arc<Mutex<MultiEngineChannelState<T>>>,
}

struct MultiEngineChannelState<T> {
    messages: VecDeque<T>,
    receiver_alive: bool,
    sender_count: usize,
    next_waker_id: usize,
    wakers: Vec<(usize, Waker)>,
}

pub struct MultiEngineChannelRecvFuture<'a, T: Send + 'static> {
    receiver: &'a MultiEngineChannelReceiver<T>,
    wait: Option<ExternalWaitGuard>,
    wait_handle: ExternalWaitHandle,
    waker_id: Option<usize>,
}

impl MultiEngineChannel {
    #[must_use]
    pub fn channel<T: Send + 'static>()
    -> (MultiEngineChannelSender<T>, MultiEngineChannelReceiver<T>) {
        let shared = Arc::new(Mutex::new(MultiEngineChannelState {
            messages: VecDeque::new(),
            receiver_alive: true,
            sender_count: 1,
            next_waker_id: 0,
            wakers: Vec::new(),
        }));

        (
            MultiEngineChannelSender {
                shared: shared.clone(),
            },
            MultiEngineChannelReceiver { shared },
        )
    }
}

impl<T: Send + 'static> Clone for MultiEngineChannelSender<T> {
    fn clone(&self) -> Self {
        self.shared.lock().unwrap().sender_count += 1;
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl<T: Send + 'static> Drop for MultiEngineChannelSender<T> {
    fn drop(&mut self) {
        let wakers = {
            let mut state = self.shared.lock().unwrap();
            state.sender_count -= 1;
            if state.sender_count == 0 {
                take_channel_wakers(&mut state)
            } else {
                Vec::new()
            }
        };

        for waker in wakers {
            waker.wake();
        }
    }
}

impl<T: Send + 'static> MultiEngineChannelSender<T> {
    pub fn send(&self, message: T) -> Result<(), T> {
        let wakers = {
            let mut state = self.shared.lock().unwrap();
            if !state.receiver_alive {
                return Err(message);
            }

            state.messages.push_back(message);
            take_channel_wakers(&mut state)
        };

        for waker in wakers {
            waker.wake();
        }

        Ok(())
    }
}

impl<T: Send + 'static> Drop for MultiEngineChannelReceiver<T> {
    fn drop(&mut self) {
        let wakers = {
            let mut state = self.shared.lock().unwrap();
            state.receiver_alive = false;
            take_channel_wakers(&mut state)
        };

        for waker in wakers {
            waker.wake();
        }
    }
}

impl<T: Send + 'static> MultiEngineChannelReceiver<T> {
    #[must_use]
    pub fn recv(&self, wait_handle: ExternalWaitHandle) -> MultiEngineChannelRecvFuture<'_, T> {
        MultiEngineChannelRecvFuture {
            receiver: self,
            wait: None,
            wait_handle,
            waker_id: None,
        }
    }
}

impl<T: Send + 'static> Future for MultiEngineChannelRecvFuture<'_, T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.receiver.shared.lock().unwrap();

        if let Some(message) = state.messages.pop_front() {
            this.waker_id = None;
            drop(state);
            drop(this.wait.take());
            return Poll::Ready(Some(message));
        }

        if state.sender_count == 0 || !state.receiver_alive {
            this.waker_id = None;
            drop(state);
            drop(this.wait.take());
            return Poll::Ready(None);
        }

        if this.wait.is_none() {
            this.wait = Some(this.wait_handle.begin_wait());
        }

        let waker_id = match this.waker_id {
            Some(waker_id) => waker_id,
            None => {
                let waker_id = state.next_waker_id;
                state.next_waker_id += 1;
                this.waker_id = Some(waker_id);
                waker_id
            }
        };

        if let Some((_, waker)) = state
            .wakers
            .iter_mut()
            .find(|(existing_id, _)| *existing_id == waker_id)
        {
            *waker = cx.waker().clone();
        } else {
            state.wakers.push((waker_id, cx.waker().clone()));
        }

        Poll::Pending
    }
}

impl<T: Send + 'static> Drop for MultiEngineChannelRecvFuture<'_, T> {
    fn drop(&mut self) {
        let Some(waker_id) = self.waker_id else {
            return;
        };

        let Ok(mut state) = self.receiver.shared.lock() else {
            return;
        };
        state
            .wakers
            .retain(|(existing_id, _)| *existing_id != waker_id);
    }
}

fn take_channel_wakers<T>(state: &mut MultiEngineChannelState<T>) -> Vec<Waker> {
    state.wakers.drain(..).map(|(_, waker)| waker).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};

    use futures::task::noop_waker;
    use gwr_track::tracker::dev_null_tracker;

    use super::*;
    use crate::engine::Engine;

    struct WakeCounter {
        wakes: Arc<AtomicUsize>,
    }

    impl Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.wakes.fetch_add(1, Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.wakes.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn counting_waker() -> (Arc<AtomicUsize>, Waker) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let waker = Waker::from(Arc::new(WakeCounter {
            wakes: wakes.clone(),
        }));
        (wakes, waker)
    }

    #[test]
    fn async_channel_receives_message_from_another_thread() {
        let tracker = dev_null_tracker();
        let engine = Engine::new(&tracker);
        let (tx, rx) = MultiEngineChannel::channel();

        let mut future = Box::pin(rx.recv(engine.external_wait_handle()));
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);

        let handle = std::thread::spawn(move || {
            tx.send(String::from("hello")).expect("send should succeed");
        });
        handle.join().expect("sender thread panicked");

        assert_eq!(
            future.as_mut().poll(&mut cx),
            Poll::Ready(Some(String::from("hello")))
        );
    }

    #[test]
    fn dropped_channel_recv_future_removes_waker() {
        let tracker = dev_null_tracker();
        let engine = Engine::new(&tracker);
        let (tx, rx) = MultiEngineChannel::channel();

        let mut future = Box::pin(rx.recv(engine.external_wait_handle()));
        let (wakes, waker) = counting_waker();
        let mut cx = Context::from_waker(&waker);

        assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
        drop(future);

        tx.send(String::from("hello")).expect("send should succeed");
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn async_sync_two_participants_returns_max_time() {
        let tracker = dev_null_tracker();

        let engine_0 = Engine::new(&tracker);
        let engine_1 = Engine::new(&tracker);

        let sync = MultiEngineSync::new(2);
        let sync_0 = sync.participant(0);
        let sync_1 = sync.participant(1);

        let mut future_0 = Box::pin(sync_0.sync(10.0, engine_0.external_wait_handle()));
        let mut future_1 = Box::pin(sync_1.sync(20.0, engine_1.external_wait_handle()));

        let noop_waker = noop_waker();
        let mut cx = Context::from_waker(&noop_waker);

        let state = future_0.as_mut().poll(&mut cx);
        assert_eq!(state, Poll::Pending);

        let state = future_1.as_mut().poll(&mut cx);
        assert_eq!(state, Poll::Ready(20.0));

        let state = future_0.as_mut().poll(&mut cx);
        assert_eq!(state, Poll::Ready(20.0));
    }

    #[test]
    fn lagging_sync_future_returns_its_generation_time() {
        let tracker = dev_null_tracker();
        let engine_0 = Engine::new(&tracker);
        let engine_1 = Engine::new(&tracker);
        let sync = MultiEngineSync::new(2);
        let sync_0 = sync.participant(0);
        let sync_1 = sync.participant(1);

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        let mut old_future_0 = Box::pin(sync_0.sync(10.0, engine_0.external_wait_handle()));
        let mut old_future_1 = Box::pin(sync_1.sync(20.0, engine_1.external_wait_handle()));
        assert_eq!(old_future_0.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(old_future_1.as_mut().poll(&mut cx), Poll::Ready(20.0));

        let mut new_future_0 = Box::pin(sync_0.sync(30.0, engine_0.external_wait_handle()));
        let mut new_future_1 = Box::pin(sync_1.sync(40.0, engine_1.external_wait_handle()));
        assert_eq!(new_future_0.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(new_future_1.as_mut().poll(&mut cx), Poll::Ready(40.0));

        assert_eq!(old_future_0.as_mut().poll(&mut cx), Poll::Ready(20.0));
        assert_eq!(new_future_0.as_mut().poll(&mut cx), Poll::Ready(40.0));
    }

    #[test]
    fn async_sync_single_participant_returns_time() {
        let tracker = dev_null_tracker();
        let engine = Engine::new(&tracker);
        let sync = MultiEngineSync::new(1);
        let sync = sync.participant(0);
        let time_ns = 42.0;

        let mut future = Box::pin(sync.sync(time_ns, engine.external_wait_handle()));
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        let result = future.as_mut().poll(&mut cx);
        assert_eq!(result, Poll::Ready(time_ns));
    }

    #[test]
    #[should_panic(expected = "sync must have at least one participant")]
    fn sync_rejects_zero_participants() {
        let _ = MultiEngineSync::new(0);
    }

    #[test]
    #[should_panic(expected = "sync participant already arrived")]
    fn sync_rejects_concurrent_arrivals_from_same_participant() {
        let tracker = dev_null_tracker();
        let engine_0 = Engine::new(&tracker);
        let engine_1 = Engine::new(&tracker);
        let sync = MultiEngineSync::new(2).participant(0);

        let mut future_0 = Box::pin(sync.sync(10.0, engine_0.external_wait_handle()));
        let mut future_1 = Box::pin(sync.sync(20.0, engine_1.external_wait_handle()));

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert_eq!(future_0.as_mut().poll(&mut cx), Poll::Pending);
        let _ = future_1.as_mut().poll(&mut cx);
    }

    #[test]
    fn dropped_sync_future_removes_arrival_and_waker() {
        let tracker = dev_null_tracker();
        let engine_0 = Engine::new(&tracker);
        let engine_1 = Engine::new(&tracker);
        let sync = MultiEngineSync::new(2);
        let sync_0 = sync.participant(0);
        let sync_1 = sync.participant(1);

        let (wakes, waker) = counting_waker();
        let mut cx = Context::from_waker(&waker);

        let mut cancelled = Box::pin(sync_0.sync(10.0, engine_0.external_wait_handle()));
        assert_eq!(cancelled.as_mut().poll(&mut cx), Poll::Pending);
        drop(cancelled);

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut future_1 = Box::pin(sync_1.sync(20.0, engine_1.external_wait_handle()));
        assert_eq!(future_1.as_mut().poll(&mut cx), Poll::Pending);

        let mut future_0 = Box::pin(sync_0.sync(30.0, engine_0.external_wait_handle()));
        assert_eq!(future_0.as_mut().poll(&mut cx), Poll::Ready(30.0));
        assert_eq!(future_1.as_mut().poll(&mut cx), Poll::Ready(30.0));
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
    }
}
