// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Flow Controlled Pipeline.
//!
//! This is a pipeline that has a buffer at one end that emits credits to the
//! other end. There is a latency for data values to travel down the pipeline
//! and a different latency can be configured for the credits to travel back to
//! the input. The size of the buffer is also configurable. For maximum
//! throughput, the buffer should be large enough to overcome the round trip
//! latency of the credit loop.
//!
//! # Ports
//!
//! This component has two ports:
//!  - One [input port](steam_engine::port::InPort): `rx`
//!  - One [output port](steam_engine::port::OutPort): `tx`

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use steam_components::delay::Delay;
use steam_components::flow_controls::credit_issuer::CreditIssuer;
use steam_components::flow_controls::credit_limiter::CreditLimiter;
use steam_components::store::Store;
use steam_components::types::Credit;
use steam_components::{connect_port, connect_tx, port_rx};
use steam_engine::executor::Spawner;
use steam_engine::port::PortState;
use steam_engine::spawn_subcomponent;
use steam_engine::time::clock::Clock;
use steam_engine::traits::SimObject;
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;

/// The Flow-Controlled Pipeline.
///
/// This `struct` shows all the building blocks of the FcPipeline.
struct FcPipelineState<T>
where
    T: SimObject,
{
    credit_limiter: RefCell<Option<CreditLimiter<T>>>,
    credit_delay: RefCell<Option<Delay<Credit>>>,
    credit_issuer: RefCell<Option<CreditIssuer<T>>>,
    data_delay: RefCell<Option<Delay<T>>>,
    buffer: RefCell<Option<Store<T>>>,
}

impl<T> FcPipelineState<T>
where
    T: SimObject,
{
    fn new(
        entity: &Arc<Entity>,
        clock: Clock,
        spawner: Spawner,
        buffer_size: usize,
        data_delay_ticks: usize,
        credit_delay_ticks: usize,
    ) -> Self {
        let credit_limiter = CreditLimiter::new(entity, spawner.clone(), buffer_size);

        let data_delay = Delay::new(
            entity,
            "pipe",
            clock.clone(),
            spawner.clone(),
            data_delay_ticks,
        );

        let buffer = Store::new(entity, "buf", spawner.clone(), buffer_size);

        connect_port!(credit_limiter, tx => data_delay, rx);
        connect_port!(data_delay, tx => buffer, rx);

        let credit_issuer = CreditIssuer::new(entity);
        let credit_delay = Delay::new(entity, "credit_pipe", clock, spawner, credit_delay_ticks);

        connect_port!(buffer, tx => credit_issuer, rx);
        connect_port!(credit_issuer, credit_tx => credit_delay, rx);
        connect_port!(credit_delay, tx => credit_limiter, credit_rx);
        Self {
            credit_limiter: RefCell::new(Some(credit_limiter)),
            credit_delay: RefCell::new(Some(credit_delay)),
            credit_issuer: RefCell::new(Some(credit_issuer)),
            data_delay: RefCell::new(Some(data_delay)),
            buffer: RefCell::new(Some(buffer)),
        }
    }
}

/// The Flow-Controlled Pipeline.
#[derive(Clone, EntityDisplay)]
pub struct FcPipeline<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    spawner: Spawner,
    state: Rc<FcPipelineState<T>>,
}

impl<T> FcPipeline<T>
where
    T: SimObject,
{
    #[must_use]
    pub fn new(
        parent: &Arc<Entity>,
        name: &str,
        clock: Clock,
        spawner: Spawner,
        buffer_size: usize,
        data_delay_ticks: usize,
        credit_delay_ticks: usize,
    ) -> Self {
        let entity = Arc::new(Entity::new(parent, name));
        Self {
            entity: entity.clone(),
            spawner: spawner.clone(),
            state: Rc::new(FcPipelineState::new(
                &entity,
                clock,
                spawner,
                buffer_size,
                data_delay_ticks,
                credit_delay_ticks,
            )),
        }
    }

    pub fn set_data_delay(&self, delay: usize) {
        self.state
            .data_delay
            .borrow()
            .as_ref()
            .unwrap()
            .set_delay(delay);
    }

    pub fn set_credit_delay(&self, delay: usize) {
        self.state
            .credit_delay
            .borrow()
            .as_ref()
            .unwrap()
            .set_delay(delay);
    }

    pub fn connect_port_tx(&mut self, port_state: Rc<PortState<T>>) {
        connect_tx!(self.state.credit_issuer, connect_port_tx ; port_state);
    }

    #[must_use]
    pub fn port_rx(&self) -> Rc<PortState<T>> {
        port_rx!(self.state.credit_limiter, port_rx)
    }

    pub async fn run(&self) -> SimResult {
        spawn_subcomponent!(self.spawner ; self.state.credit_limiter);
        spawn_subcomponent!(self.spawner ; self.state.credit_delay);
        spawn_subcomponent!(self.spawner ; self.state.credit_issuer);
        spawn_subcomponent!(self.spawner ; self.state.data_delay);
        spawn_subcomponent!(self.spawner ; self.state.buffer);
        Ok(())
    }
}
