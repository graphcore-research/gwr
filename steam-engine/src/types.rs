// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Shared types.

use std::error::Error;
use std::fmt;

use crate::traits::Event;

/// The return value from a call to [listen()](crate::traits::Event)
pub type EventResult<T> = T;

pub type Eventable<T> = Box<dyn Event<T> + 'static>;

// Simulation errors

#[macro_export]
/// Build a [SimError] from a message that supports `to_string`
macro_rules! sim_error {
    ($msg:expr) => {
        Err($crate::types::SimError($msg.to_string()))?
    };
}

/// The `SimError` is what should be returned in the case of an error
#[derive(Debug)]
pub struct SimError(pub String);

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Error: {}", self.0)
    }
}

impl Error for SimError {}

/// The SimResult is the return type for most simulation functions
pub type SimResult = Result<(), SimError>;

// Generic packet types
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum ReqType {
    #[default]
    Read,
    Write,
    WriteNonPosted,
    Control,
}

impl fmt::Display for ReqType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ReqType::Read => {
                write!(f, "Read")
            }
            ReqType::Write => {
                write!(f, "Write")
            }
            ReqType::WriteNonPosted => {
                write!(f, "WriteNonPosted")
            }
            ReqType::Control => {
                write!(f, "Control")
            }
        }
    }
}
