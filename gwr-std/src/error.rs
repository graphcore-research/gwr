// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Simulation errors

use std::error::Error;
use std::fmt;

/// Build a [SimError] from a message that supports `to_string`
#[macro_export]
macro_rules! sim_error {
    ($($arg:tt)+) => {
        Err($crate::error::SimError(format!($($arg)+).to_string()))
    };
}

/// Error returned while parsing, constructing, connecting, or running a
/// simulation.
///
/// Prefer catching static mistakes early. Use parse-time validation when
/// reading files, and construction-time and connect-time validation where
/// possible.
///
/// Include enough detail to identify the entity, config item, port, node, or
/// edge that caused the failure clearly enough to fix the model topology or
/// input.
#[derive(Debug)]
pub struct SimError(pub String);

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for SimError {}

/// The SimResult is the return type for most simulation functions
pub type SimResult = Result<(), SimError>;
