// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

//! Port

use std::cell::RefCell;
use std::fmt;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use futures::Future;
use futures::future::FusedFuture;
use gwr_track::connect;
use gwr_track::entity::{Entity, GetEntity};
use gwr_track::tracker::aka::Aka;

use crate::engine::Engine;
use crate::port::monitor::Monitor;
use crate::sim_error;
use crate::time::clock::Clock;
use crate::traits::SimObject;
use crate::types::{SimError, SimResult};

pub mod monitor;

pub type PortStateResult<T> = Result<Rc<PortState<T>>, SimError>;
pub type PortGetResult<T> = Result<PortGet<T>, SimError>;
pub type PortStartGetResult<T> = Result<PortStartGet<T>, SimError>;
pub type PortPutResult<T> = Result<PortPut<T>, SimError>;
pub type PortTryPutResult<T> = Result<PortTryPut<T>, SimError>;

pub struct PortState<T>
where
    T: SimObject,
{
    value: RefCell<Option<T>>,
    waiting_get: RefCell<Option<Waker>>,
    waiting_put: RefCell<Option<Waker>>,
    pub in_port_entity: Rc<Entity>,
    monitor: Option<Rc<Monitor>>,
}

impl<T> PortState<T>
where
    T: SimObject,
{
    fn new(
        engine: &Engine,
        clock: &Clock,
        in_port_entity: Rc<Entity>,
        window_size_ticks: Option<u64>,
    ) -> Self {
        let monitor = window_size_ticks.map(|window_size_ticks| {
            Monitor::new_and_register(engine, &in_port_entity, clock, window_size_ticks)
        });
        Self {
            value: RefCell::new(None),
            waiting_get: RefCell::new(None),
            waiting_put: RefCell::new(None),
            in_port_entity,
            monitor,
        }
    }
}

pub struct InPort<T>
where
    T: SimObject,
{
    entity: Rc<Entity>,
    state: Rc<PortState<T>>,
    connected: RefCell<bool>,
}

impl<T> fmt::Display for InPort<T>
where
    T: SimObject,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.entity.fmt(f)
    }
}

impl<T> InPort<T>
where
    T: SimObject,
{
    #[must_use]
    pub fn new(engine: &Engine, clock: &Clock, parent: &Rc<Entity>, name: &str) -> Self {
        Self::new_with_renames(engine, clock, parent, name, None)
    }

    #[must_use]
    pub fn new_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
    ) -> Self {
        let entity = Rc::new(Entity::new_with_renames(parent, name, aka));
        let monitor_window_size = entity.tracker.monitoring_window_size_for(entity.id);
        Self {
            entity: entity.clone(),
            state: Rc::new(PortState::new(engine, clock, entity, monitor_window_size)),
            connected: RefCell::new(false),
        }
    }

    pub fn state(&self) -> PortStateResult<T> {
        if *self.connected.borrow() {
            return sim_error!("{self} already connected");
        }

        *self.connected.borrow_mut() = true;
        Ok(self.state.clone())
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn get(&self) -> PortGetResult<T> {
        if !*self.connected.borrow() {
            return sim_error!("{self} not connected");
        }

        Ok(PortGet {
            state: self.state.clone(),
            done: false,
        })
    }

    /// Must be matched with a `finish_get` to allow the OutPort to continue.
    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn start_get(&self) -> PortStartGetResult<T> {
        if !*self.connected.borrow() {
            return sim_error!("{self} not connected");
        }

        Ok(PortStartGet {
            state: self.state.clone(),
            done: false,
        })
    }

    /// Must be matched with a `start_get ` to consume the value.
    pub fn finish_get(&self) {
        if let Some(waker) = self.state.waiting_put.borrow_mut().take() {
            waker.wake();
        }
    }
}

pub struct OutPort<T>
where
    T: SimObject,
{
    entity: Rc<Entity>,
    state: Option<Rc<PortState<T>>>,
}

impl<T> GetEntity for OutPort<T>
where
    T: SimObject,
{
    fn entity(&self) -> &Rc<Entity> {
        &self.entity
    }
}

impl<T> fmt::Display for OutPort<T>
where
    T: SimObject,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.entity.fmt(f)
    }
}

impl<T> OutPort<T>
where
    T: SimObject,
{
    #[must_use]
    pub fn new(parent: &Rc<Entity>, name: &str) -> Self {
        Self::new_with_renames(parent, name, None)
    }

    #[must_use]
    pub fn new_with_renames(parent: &Rc<Entity>, name: &str, aka: Option<&Aka>) -> Self {
        let entity = Rc::new(Entity::new_with_renames(parent, name, aka));
        Self {
            entity,
            state: None,
        }
    }

    pub fn connect(&mut self, port_state: PortStateResult<T>) -> SimResult {
        let port_state = port_state?;

        connect!(self.entity ; port_state.in_port_entity);
        match self.state {
            Some(_) => {
                return sim_error!("{self} already connected");
            }
            None => {
                self.state = Some(port_state);
            }
        }
        Ok(())
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn put(&self, value: T) -> PortPutResult<T> {
        let state = match self.state.as_ref() {
            Some(s) => s.clone(),
            None => return sim_error!("{self} not connected"),
        };
        Ok(PortPut {
            state,
            value: Some(value),
            done: false,
        })
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn try_put(&self) -> PortTryPutResult<T> {
        let state = match self.state.as_ref() {
            Some(s) => s.clone(),
            None => return sim_error!("{self} not connected"),
        };
        Ok(PortTryPut { state, done: false })
    }
}

pub struct PortPut<T>
where
    T: SimObject,
{
    state: Rc<PortState<T>>,
    value: Option<T>,
    done: bool,
}

impl<T> Future for PortPut<T>
where
    T: SimObject,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.value.take() {
            Some(value) => {
                // The state is designed to be shared between one put/get pair so it should
                // not be possible for the value in the state to be set at this point.
                assert!(self.state.value.borrow().is_none());

                *self.state.value.borrow_mut() = Some(value);
                if let Some(waker) = self.state.waiting_get.borrow_mut().take() {
                    waker.wake();
                }
                *self.state.waiting_put.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }
            None => {
                // Value already sent, woken because it has been consumed
                self.done = true;
                Poll::Ready(())
            }
        }
    }
}

impl<T> FusedFuture for PortPut<T>
where
    T: SimObject,
{
    fn is_terminated(&self) -> bool {
        self.done
    }
}

pub struct PortTryPut<T>
where
    T: SimObject,
{
    state: Rc<PortState<T>>,
    done: bool,
}

impl<T> Future for PortTryPut<T>
where
    T: SimObject,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.state.waiting_get.borrow().is_some() {
            self.done = true;
            Poll::Ready(())
        } else {
            *self.state.waiting_put.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl<T> FusedFuture for PortTryPut<T>
where
    T: SimObject,
{
    fn is_terminated(&self) -> bool {
        self.done
    }
}

pub struct PortGet<T>
where
    T: SimObject,
{
    state: Rc<PortState<T>>,
    done: bool,
}

impl<T> Future for PortGet<T>
where
    T: SimObject,
{
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let value = self.state.value.borrow_mut().take();
        if let Some(value) = value {
            self.done = true;
            self.state.waiting_get.borrow_mut().take();

            // Track the object through the port monitor if there is one
            if let Some(monitor) = self.state.monitor.as_ref() {
                monitor.sample(&value);
            }

            if let Some(waker) = self.state.waiting_put.borrow_mut().take() {
                waker.wake();
            }
            Poll::Ready(value)
        } else {
            if let Some(waker) = self.state.waiting_put.borrow_mut().take() {
                waker.wake();
            }

            *self.state.waiting_get.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl<T> FusedFuture for PortGet<T>
where
    T: SimObject,
{
    fn is_terminated(&self) -> bool {
        self.done
    }
}

pub struct PortStartGet<T>
where
    T: SimObject,
{
    state: Rc<PortState<T>>,
    done: bool,
}

impl<T> Future for PortStartGet<T>
where
    T: SimObject,
{
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let value = self.state.value.borrow_mut().take();
        if let Some(value) = value {
            self.done = true;
            self.state.waiting_get.borrow_mut().take();

            // Track the object through the port monitor if there is one
            if let Some(monitor) = self.state.monitor.as_ref() {
                monitor.sample(&value);
            }

            Poll::Ready(value)
        } else {
            *self.state.waiting_get.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl<T> FusedFuture for PortStartGet<T>
where
    T: SimObject,
{
    fn is_terminated(&self) -> bool {
        self.done
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Wake, Waker};

    use futures::future::FusedFuture;
    use futures::task::noop_waker;
    use gwr_track::Tracker;
    use gwr_track::entity::Entity;
    use gwr_track::tracker::dev_null_tracker;

    use super::*;
    use crate::traits::TotalBytes;

    struct TestContext {
        // Just kept to ensure it isn't dropped
        _tracker: Tracker,
        engine: Engine,
        clock: Clock,
    }

    fn test_context() -> TestContext {
        let tracker = dev_null_tracker();
        let mut engine = Engine::new(&tracker);
        let clock = engine.default_clock();

        TestContext {
            _tracker: tracker,
            engine,
            clock,
        }
    }

    fn test_state<T: SimObject>() -> Rc<PortState<T>> {
        let context = test_context();
        let entity = Rc::new(Entity::new(context.engine.top(), "rx"));

        Rc::new(PortState::new(
            &context.engine,
            &context.clock,
            entity,
            None,
        ))
    }

    fn monitored_test_state<T: SimObject>() -> Rc<PortState<T>> {
        let context = test_context();
        let entity = Rc::new(Entity::new(context.engine.top(), "rx"));

        Rc::new(PortState::new(
            &context.engine,
            &context.clock,
            entity,
            Some(1),
        ))
    }

    struct WakeCounter {
        wakes_count: Arc<AtomicUsize>,
    }

    impl Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.wakes_count.fetch_add(1, Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.wakes_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn counting_waker() -> (Arc<AtomicUsize>, Waker) {
        let wakes_count = Arc::new(AtomicUsize::new(0));
        let waker = Waker::from(Arc::new(WakeCounter {
            wakes_count: wakes_count.clone(),
        }));

        (wakes_count, waker)
    }

    #[test]
    fn wake_counter_counts_wake_and_wake_by_ref() {
        let (wakes_count, waker) = counting_waker();

        waker.wake_by_ref();
        assert_eq!(wakes_count.load(Ordering::SeqCst), 1);

        waker.wake();
        assert_eq!(wakes_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn in_port_state_can_only_connect_once() {
        let context = test_context();
        let in_port =
            InPort::<i32>::new(&context.engine, &context.clock, context.engine.top(), "rx");

        assert!(in_port.state().is_ok());

        let err = in_port
            .state()
            .err()
            .expect("second state call should fail");
        assert!(format!("{err}").contains("already connected"));
    }

    #[test]
    fn out_port_connect_can_only_connect_once() {
        let context = test_context();
        let mut out_port = OutPort::<i32>::new(context.engine.top(), "tx");
        let first_in_port =
            InPort::new(&context.engine, &context.clock, context.engine.top(), "rx1");
        let second_in_port =
            InPort::new(&context.engine, &context.clock, context.engine.top(), "rx2");

        out_port.connect(first_in_port.state()).unwrap();

        let err = out_port.connect(second_in_port.state()).unwrap_err();
        assert!(format!("{err}").contains("already connected"));
    }

    #[test]
    fn out_port_entity_returns_port_entity() {
        let context = test_context();
        let out_port = OutPort::<i32>::new(context.engine.top(), "tx");

        assert!(Rc::ptr_eq(out_port.entity(), &out_port.entity));
    }

    #[test]
    fn start_get_requires_connection_and_finish_get_wakes_putter() {
        let context = test_context();
        let in_port =
            InPort::<i32>::new(&context.engine, &context.clock, context.engine.top(), "rx");

        assert!(in_port.start_get().is_err());
        assert!(in_port.state().is_ok());
        assert!(in_port.start_get().is_ok());

        let waker = noop_waker();
        *in_port.state.waiting_put.borrow_mut() = Some(waker);
        in_port.finish_get();

        assert!(in_port.state.waiting_put.borrow().is_none());
    }

    #[test]
    fn finish_get_without_waiting_putter_is_a_noop() {
        let context = test_context();
        let in_port =
            InPort::<i32>::new(&context.engine, &context.clock, context.engine.top(), "rx");

        in_port.finish_get();

        assert!(in_port.state.waiting_put.borrow().is_none());
    }

    #[test]
    fn port_put_reports_termination_after_second_poll() {
        let state = test_state::<i32>();
        let put = PortPut {
            state: state.clone(),
            value: Some(123),
            done: false,
        };
        let mut put = Box::pin(put);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert_eq!(put.as_mut().poll(&mut cx), Poll::Pending);
        assert!(!put.is_terminated());
        assert_eq!(*state.value.borrow(), Some(123));
        assert!(state.waiting_put.borrow().is_some());

        assert_eq!(put.as_mut().poll(&mut cx), Poll::Ready(()));
        assert!(put.is_terminated());
    }

    #[test]
    fn port_try_put_waits_for_getter_then_completes() {
        let state = test_state::<i32>();
        let try_put = PortTryPut {
            state: state.clone(),
            done: false,
        };
        let mut try_put = Box::pin(try_put);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert_eq!(try_put.as_mut().poll(&mut cx), Poll::Pending);
        assert!(!try_put.is_terminated());
        assert!(state.waiting_put.borrow().is_some());

        *state.waiting_get.borrow_mut() = Some(noop_waker());

        assert_eq!(try_put.as_mut().poll(&mut cx), Poll::Ready(()));
        assert!(try_put.is_terminated());
    }

    #[test]
    fn connected_out_port_creates_try_put_future() {
        let context = test_context();
        let mut out_port = OutPort::<i32>::new(context.engine.top(), "tx");
        let in_port = InPort::new(&context.engine, &context.clock, context.engine.top(), "rx");

        out_port.connect(in_port.state()).unwrap();

        assert!(out_port.try_put().is_ok());
    }

    #[test]
    fn port_get_waits_then_returns_value_and_reports_termination() {
        let state = test_state::<i32>();
        let get = PortGet {
            state: state.clone(),
            done: false,
        };
        let mut get = Box::pin(get);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert_eq!(get.as_mut().poll(&mut cx), Poll::Pending);
        assert!(!get.is_terminated());
        assert!(state.waiting_get.borrow().is_some());

        *state.value.borrow_mut() = Some(456);
        *state.waiting_put.borrow_mut() = Some(noop_waker());

        assert_eq!(get.as_mut().poll(&mut cx), Poll::Ready(456));
        assert!(get.is_terminated());
        assert!(state.waiting_put.borrow().is_none());
    }

    #[test]
    fn port_get_pending_wakes_waiting_putter() {
        let state = test_state::<i32>();
        let get = PortGet {
            state: state.clone(),
            done: false,
        };
        let mut get = Box::pin(get);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        *state.waiting_put.borrow_mut() = Some(noop_waker());

        assert_eq!(get.as_mut().poll(&mut cx), Poll::Pending);

        assert!(state.waiting_put.borrow().is_none());
        assert!(state.waiting_get.borrow().is_some());
    }

    #[test]
    fn port_get_samples_monitored_values() {
        let state = monitored_test_state::<i32>();
        let monitor = state
            .monitor
            .as_ref()
            .expect("monitored state should create a monitor");
        let get = PortGet {
            state: state.clone(),
            done: false,
        };
        let mut get = Box::pin(get);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        *state.value.borrow_mut() = Some(456);

        assert_eq!(get.as_mut().poll(&mut cx), Poll::Ready(456));
        assert_eq!(monitor.bytes_in_window(), 456_i32.total_bytes());
    }

    #[test]
    fn port_start_get_waits_then_returns_value_without_finishing_put() {
        let state = test_state::<i32>();
        let start_get = PortStartGet {
            state: state.clone(),
            done: false,
        };
        let mut start_get = Box::pin(start_get);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert_eq!(start_get.as_mut().poll(&mut cx), Poll::Pending);
        assert!(!start_get.is_terminated());
        assert!(state.waiting_get.borrow().is_some());

        let (waiting_put_wakes, waiting_put_waker) = counting_waker();
        *state.waiting_put.borrow_mut() = Some(waiting_put_waker.clone());
        *state.value.borrow_mut() = Some(789);

        assert_eq!(start_get.as_mut().poll(&mut cx), Poll::Ready(789));
        assert!(start_get.is_terminated());
        assert!(state.waiting_get.borrow().is_none());
        assert_eq!(waiting_put_wakes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn port_start_get_samples_monitored_values() {
        let state = monitored_test_state::<i32>();
        let monitor = state
            .monitor
            .as_ref()
            .expect("monitored state should create a monitor");
        let start_get = PortStartGet {
            state: state.clone(),
            done: false,
        };
        let mut start_get = Box::pin(start_get);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        *state.value.borrow_mut() = Some(789);

        assert_eq!(start_get.as_mut().poll(&mut cx), Poll::Ready(789));
        assert_eq!(monitor.bytes_in_window(), 789_i32.total_bytes());
    }
}
