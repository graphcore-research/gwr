// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::SimObject;
use gwr_engine::types::SimError;
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;

use super::Store;

/// Builds stores that support a configurable number of objects.
///
/// The returned [Store] is the registered component.
pub struct ObjectStore<T>(std::marker::PhantomData<T>);

impl<T> ObjectStore<T>
where
    T: SimObject,
{
    /// Basic store constructor
    ///
    /// Returns a `SimError` if `capacity` is 0.
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        capacity: usize,
    ) -> Result<Rc<Store<T>>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));
        let store = Rc::new(Store::new(
            engine,
            clock,
            &entity,
            aka,
            capacity,
            "objects",
            |_| 1,
        )?);
        engine.register(store.clone());
        Ok(store)
    }

    /// Basic store constructor
    ///
    /// Returns a `SimError` if `capacity` is 0.
    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        capacity: usize,
    ) -> Result<Rc<Store<T>>, SimError> {
        Self::new_and_register_with_renames(engine, clock, parent, name, None, capacity)
    }
}
