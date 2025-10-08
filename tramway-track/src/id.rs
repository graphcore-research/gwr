// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Id

/// IDs that should be unique across the simulation
///
/// Each _log_/_trace_ event within the application is given a unique ID to
/// identify it. There are two reserved ID values: [NO_ID](constant.NO_ID.html)
/// and [ROOT](constant.ROOT.html)
#[derive(Copy, Clone, Default, Eq, Hash, PartialEq)]
pub struct Id(pub u64);

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Debug for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for Id {
    fn from(val: &str) -> Self {
        let value = val.parse::<u64>().unwrap();
        Id(value)
    }
}

/// The `Unique` trait provides a unique ID for logging
pub trait Unique {
    /// Return a unique ID for an object.
    fn id(&self) -> Id;
}

impl Unique for Id {
    fn id(&self) -> Id {
        *self
    }
}

// Provide Unique for primitive types
impl Unique for i32 {
    fn id(&self) -> Id {
        Id(*self as u64)
    }
}

impl Unique for usize {
    fn id(&self) -> Id {
        Id(*self as u64)
    }
}
