// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! DataFrame provides a configurable data frame
//!
//! The user can specify the number of payload bytes as well as the number
//! of protocol overhead bytes.

use std::fmt::Display;
use std::rc::Rc;

use gwr_engine::traits::{SimObject, TotalBytes};
use gwr_track::entity::Entity;
use gwr_track::id::Unique;
use gwr_track::{Id, create, create_id};

#[derive(Clone, Debug)]
pub struct DataFrame {
    created_by: Rc<Entity>,
    id: Id,
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
        };
        create!(created_by ; frame, frame.total_bytes());
        frame
    }
}

impl SimObject for DataFrame {}

impl Display for DataFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}: {:?} ({},{} bytes)",
            self.created_by, self.id, self.overhead_size_bytes, self.payload_size_bytes
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
