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

use async_trait::async_trait;
use steam_components::delay::Delay;
use steam_components::flow_controls::credit_issuer::CreditIssuer;
use steam_components::flow_controls::credit_limiter::CreditLimiter;
use steam_components::store::Store;
use steam_components::types::Credit;
use steam_components::{connect_port, connect_tx, port_rx};
use steam_engine::engine::Engine;
use steam_engine::executor::Spawner;
use steam_engine::port::PortStateResult;
use steam_engine::time::clock::Clock;
use steam_engine::traits::SimObject;
use steam_engine::types::{SimError, SimResult};
use steam_model_builder::{EntityDisplay, Runnable};
use steam_track::entity::Entity;

/// Configuration for a flow-controlled pipeline.
pub struct FcPipelineConfig {
    buffer_size: usize,
    data_delay_ticks: usize,
    credit_delay_ticks: usize,
}

impl FcPipelineConfig {
    #[must_use]
    pub fn new(buffer_size: usize, data_delay_ticks: usize, credit_delay_ticks: usize) -> Self {
        Self {
            buffer_size,
            data_delay_ticks,
            credit_delay_ticks,
        }
    }
}

/// The Flow-Controlled Pipeline.
#[derive(EntityDisplay, Runnable)]
pub struct FcPipeline<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    credit_limiter: RefCell<Option<Rc<CreditLimiter<T>>>>,
    credit_delay: RefCell<Option<Rc<Delay<Credit>>>>,
    credit_issuer: RefCell<Option<Rc<CreditIssuer<T>>>>,
    data_delay: RefCell<Option<Rc<Delay<T>>>>,
}

impl<T> FcPipeline<T>
where
    T: SimObject,
{
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        clock: Clock,
        spawner: Spawner,
        config: &FcPipelineConfig,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Arc::new(Entity::new(parent, name));

        let credit_limiter =
            CreditLimiter::new_and_register(engine, &entity, spawner.clone(), config.buffer_size)?;

        let data_delay = Delay::new_and_register(
            engine,
            &entity,
            "pipe",
            clock.clone(),
            spawner.clone(),
            config.data_delay_ticks,
        )?;
        // The whole point of the flow-controlled pipeline is that the delays should
        // never have to stall at their outputs
        data_delay.set_error_on_output_stall();

        let buffer =
            Store::new_and_register(engine, &entity, "buf", spawner.clone(), config.buffer_size)?;

        connect_port!(credit_limiter, tx => data_delay, rx)?;
        connect_port!(data_delay, tx => buffer, rx)?;

        let credit_issuer = CreditIssuer::new_and_register(engine, &entity)?;
        let credit_delay = Delay::new_and_register(
            engine,
            &entity,
            "credit_pipe",
            clock,
            spawner,
            config.credit_delay_ticks,
        )?;
        // The whole point of the flow-controlled pipeline is that the delays should
        // never have to stall at their outputs
        credit_delay.set_error_on_output_stall();

        connect_port!(buffer, tx => credit_issuer, rx)?;
        connect_port!(credit_issuer, credit_tx => credit_delay, rx)?;
        connect_port!(credit_delay, tx => credit_limiter, credit_rx)?;

        let rc_self = Rc::new(Self {
            entity,
            credit_limiter: RefCell::new(Some(credit_limiter)),
            credit_delay: RefCell::new(Some(credit_delay)),
            credit_issuer: RefCell::new(Some(credit_issuer)),
            data_delay: RefCell::new(Some(data_delay)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn set_data_delay(&self, delay: usize) -> SimResult {
        self.data_delay.borrow().as_ref().unwrap().set_delay(delay)
    }

    pub fn set_credit_delay(&self, delay: usize) -> SimResult {
        self.credit_delay
            .borrow()
            .as_ref()
            .unwrap()
            .set_delay(delay)
    }

    pub fn connect_port_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        connect_tx!(self.credit_issuer, connect_port_tx ; port_state)
    }

    pub fn port_rx(&self) -> PortStateResult<T> {
        port_rx!(self.credit_limiter, port_rx)
    }
}
