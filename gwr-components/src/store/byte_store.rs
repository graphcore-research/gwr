// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::SimObject;
use gwr_engine::types::SimError;
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;

use super::Store;

/// Builds stores that support a configurable number of bytes.
///
/// Objects must support the [SimObject] trait. The returned [Store] uses
/// [`total_bytes`](gwr_engine::traits::TotalBytes::total_bytes) to decide
/// whether there is enough free byte capacity to accept each object.
pub struct ByteStore<T>(std::marker::PhantomData<T>);

impl<T> ByteStore<T>
where
    T: SimObject,
{
    /// Basic byte-store constructor.
    ///
    /// Returns a `SimError` if `capacity_bytes` is 0.
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        capacity_bytes: usize,
    ) -> Result<Rc<Store<T>>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));
        let store = Rc::new(Store::new(
            engine,
            clock,
            &entity,
            aka,
            capacity_bytes,
            "bytes",
            |value: &T| value.total_bytes(),
        )?);
        engine.register(store.clone());
        Ok(store)
    }

    /// Basic byte-store constructor.
    ///
    /// Returns a `SimError` if `capacity_bytes` is 0.
    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        capacity_bytes: usize,
    ) -> Result<Rc<Store<T>>, SimError> {
        Self::new_and_register_with_renames(engine, clock, parent, name, None, capacity_bytes)
    }
}
