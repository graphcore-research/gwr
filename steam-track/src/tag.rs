// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Tag

/// Tags that should be unique across the simulation
///
/// Each _log_/_trace_ event within the application is given a unique tag to
/// identify it. There are two reserved tag values: [NO_ID](constant.NO_ID.html)
/// and [ROOT](constant.ROOT.html)
#[derive(Copy, Clone, Default, Eq, Hash, PartialEq)]
pub struct Tag(pub u64);

impl std::fmt::Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Debug for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The `Tagged` trait provides an interface to an object to enable it to be
/// routed
pub trait Tagged {
    /// Return a unique tag for an object. This is used in logging.
    fn tag(&self) -> Tag;
}

impl Tagged for Tag {
    fn tag(&self) -> Tag {
        *self
    }
}

// Provide Tagged for primitive types
impl Tagged for i32 {
    fn tag(&self) -> Tag {
        Tag(*self as u64)
    }
}

impl Tagged for usize {
    fn tag(&self) -> Tag {
        Tag(*self as u64)
    }
}
