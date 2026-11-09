// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Shared types.

use std::fmt;
use std::rc::Rc;

use crate::traits::{Event, Runnable};

/// The return value from a call to [listen()](crate::traits::Event)
pub type EventResult<T> = T;

pub type Eventable<T> = Box<dyn Event<T> + 'static>;

/// The type of a component that can be registered with the `Engine` so that it
/// will automatically be spawned.
pub type Component = Rc<dyn Runnable + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub u64);

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Generic access types
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum AccessType {
    #[default]
    ReadRequest,
    WriteRequest,
    WriteNonPostedRequest,
    ReadResponse,
    WriteNonPostedResponse,
    Control,
    BarrierRequest,
    BarrierResponse,
}

impl fmt::Display for AccessType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            AccessType::ReadRequest => {
                write!(f, "ReadRequest")
            }
            AccessType::WriteRequest => {
                write!(f, "WriteRequest")
            }
            AccessType::WriteNonPostedRequest => {
                write!(f, "WriteNonPostedRequest")
            }
            AccessType::ReadResponse => {
                write!(f, "ReadResponse")
            }
            AccessType::WriteNonPostedResponse => {
                write!(f, "WriteNonPostedResponse")
            }
            AccessType::BarrierRequest => write!(f, "BarrierRequest"),
            AccessType::BarrierResponse => write!(f, "BarrierResponse"),
            AccessType::Control => {
                write!(f, "Control")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test added for code coverage
    #[test]
    fn access_type_display_names_match_variants() {
        assert_eq!(AccessType::ReadRequest.to_string(), "ReadRequest");
        assert_eq!(AccessType::WriteRequest.to_string(), "WriteRequest");
        assert_eq!(
            AccessType::WriteNonPostedRequest.to_string(),
            "WriteNonPostedRequest"
        );
        assert_eq!(AccessType::ReadResponse.to_string(), "ReadResponse");
        assert_eq!(
            AccessType::WriteNonPostedResponse.to_string(),
            "WriteNonPostedResponse"
        );
        assert_eq!(AccessType::Control.to_string(), "Control");
    }
}
