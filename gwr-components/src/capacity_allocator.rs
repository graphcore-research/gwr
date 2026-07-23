// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Capacity accounting and scoped reservations.

use std::cell::RefCell;
use std::rc::Rc;

use gwr_engine::events::repeated::Repeated;
use gwr_engine::sim_error;
use gwr_engine::traits::Event;
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::entity::Entity;

/// A reusable capacity allocator for object counts, bytes, or other units.
#[derive(Clone, EntityGet, EntityDisplay)]
pub struct CapacityAllocator {
    entity: Rc<Entity>,
    capacity: usize,
    capacity_unit: Rc<String>,
    used: Rc<RefCell<usize>>,
    level_change: Repeated<usize>,
}

pub struct CapacityReservation {
    allocator: CapacityAllocator,
    units: usize,
}

impl Drop for CapacityReservation {
    fn drop(&mut self) {
        self.allocator.release(self.units);
    }
}

impl CapacityAllocator {
    /// Create a standalone allocator with its own child entity.
    pub fn new(
        parent: &Rc<Entity>,
        name: &str,
        capacity: usize,
        capacity_unit: impl Into<String>,
    ) -> Result<Self, SimError> {
        let entity = Rc::new(Entity::new(parent, name));
        Self::for_entity(&entity, capacity, capacity_unit)
    }

    /// Create allocator bookkeeping on an existing entity.
    pub fn for_entity(
        entity: &Rc<Entity>,
        capacity: usize,
        capacity_unit: impl Into<String>,
    ) -> Result<Self, SimError> {
        if capacity == 0 {
            return sim_error!("Unsupported CapacityAllocator with capacity of 0");
        }
        let capacity_unit = capacity_unit.into();
        entity.track_capacity(capacity, &capacity_unit);

        Ok(Self {
            entity: entity.clone(),
            capacity,
            capacity_unit: Rc::new(capacity_unit),
            used: Rc::new(RefCell::new(0)),
            level_change: Repeated::new(usize::default()),
        })
    }

    #[must_use]
    pub fn used(&self) -> usize {
        *self.used.borrow()
    }

    #[must_use]
    pub fn has_capacity_for(&self, units: usize) -> bool {
        units <= self.capacity - self.used()
    }

    pub fn check_units_can_fit(&self, units: usize) -> SimResult {
        if units > self.capacity {
            return sim_error!(
                "Cannot allocate {units} {} in {:?} with capacity {}",
                self.capacity_unit,
                self.entity.full_name(),
                self.capacity
            );
        }
        Ok(())
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[must_use]
    pub fn capacity_unit(&self) -> &str {
        self.capacity_unit.as_str()
    }

    #[must_use]
    pub fn level_change_event(&self) -> Repeated<usize> {
        self.level_change.clone()
    }

    pub async fn wait_for_capacity(&self, units: usize) -> SimResult {
        self.check_units_can_fit(units)?;
        let level_change = self.level_change_event();
        while !self.has_capacity_for(units) {
            level_change.listen().await;
        }
        Ok(())
    }

    pub fn allocate(&self, units: usize) -> SimResult {
        self.check_units_can_fit(units)?;
        if !self.has_capacity_for(units) {
            return sim_error!("Overflow in {:?}", self.entity.full_name());
        }

        let used = {
            let mut used = self.used.borrow_mut();
            *used += units;
            *used
        };
        self.level_change.notify_result(used);
        Ok(())
    }

    pub fn release(&self, units: usize) {
        let used = {
            let mut used = self.used.borrow_mut();
            *used = used
                .checked_sub(units)
                .expect("capacity allocator underflow");
            *used
        };
        self.level_change.notify_result(used);
    }

    pub async fn reserve(&self, units: usize) -> Result<CapacityReservation, SimError> {
        self.wait_for_capacity(units).await?;
        self.allocate(units)?;
        Ok(CapacityReservation {
            allocator: self.clone(),
            units,
        })
    }
}
