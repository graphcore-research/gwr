// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! DataFrame provides a configurable data frame
//!
//! The user can specify the number of payload bytes as well as the number
//! of protocol overhead bytes.

use std::fmt::Display;
use std::rc::Rc;

use gwr_engine::traits::{Routable, SimObject, TotalBytes};
use gwr_engine::types::AccessType;
use gwr_track::entity::Entity;
use gwr_track::id::Unique;
use gwr_track::{Id, create, create_id};

#[derive(Clone, Debug)]
pub struct DataFrame {
    created_by: Rc<Entity>,
    id: Id,
    dest: u64,
    /// User-set value to aid debug tracking
    label: u64,
    access_type: AccessType,
    payload_size_bytes: usize,
    overhead_size_bytes: usize,
}

impl DataFrame {
    #[must_use]
    pub fn new(
        created_by: &Rc<Entity>,
        overhead_size_bytes: usize,
        payload_size_bytes: usize,
    ) -> Self {
        let frame = Self {
            created_by: created_by.clone(),
            id: create_id!(created_by),
            payload_size_bytes,
            overhead_size_bytes,
            label: 0,
            dest: 0,
            access_type: AccessType::Control,
        };
        create!(created_by ; frame, frame.total_bytes());
        frame
    }

    #[must_use]
    pub fn set_label(mut self, label: u64) -> Self {
        self.label = label;
        self
    }

    #[must_use]
    pub fn set_dest(mut self, dest: u64) -> Self {
        self.dest = dest;
        self
    }

    #[must_use]
    pub fn set_access_type(mut self, access_type: AccessType) -> Self {
        self.access_type = access_type;
        self
    }
}

impl SimObject for DataFrame {}

impl Display for DataFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}: 0x{:x} -> 0x{:x} ({},{} bytes)",
            self.created_by,
            self.label,
            self.dest,
            self.overhead_size_bytes,
            self.payload_size_bytes
        )
    }
}

impl TotalBytes for DataFrame {
    fn total_bytes(&self) -> usize {
        self.payload_size_bytes + self.overhead_size_bytes
    }
}

impl Unique for DataFrame {
    fn id(&self) -> Id {
        self.id
    }
}

impl Routable for DataFrame {
    fn access_type(&self) -> gwr_engine::types::AccessType {
        self.access_type
    }
    fn destination(&self) -> u64 {
        self.dest
    }
}

/// Allow Box of any SimObject type to be used
impl SimObject for Box<DataFrame> {}

impl TotalBytes for Box<DataFrame> {
    fn total_bytes(&self) -> usize {
        self.as_ref().total_bytes()
    }
}

impl Unique for Box<DataFrame> {
    fn id(&self) -> gwr_track::Id {
        self.as_ref().id()
    }
}
