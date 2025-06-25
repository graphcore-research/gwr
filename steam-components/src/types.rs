// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Shared types.
//!
//! This file defines a number of common types used to connect blocks.

use std::mem::size_of;

use steam_engine::traits::{Routable, SimObject, TotalBytes};
use steam_engine::types::{ReqType, SimError};
use steam_track::tag::Tagged;

/// The `DataGenerator` is what a [source](crate::source) uses
/// to generate data values to send.
pub type DataGenerator<T> = Box<dyn Iterator<Item = T> + 'static>;

/// The return value from a call to [get()](crate::types::GetResult)
///
/// It can either return a value or a [SimError].
pub type GetResult<T> = Result<T, SimError>;

#[derive(Clone, Debug)]
pub struct Credit(pub usize);

impl TotalBytes for Credit {
    fn total_bytes(&self) -> usize {
        size_of::<usize>()
    }
}

impl Routable for Credit {
    fn dest(&self) -> u64 {
        panic!("Cannot route Credit");
    }
    fn req_type(&self) -> ReqType {
        ReqType::Control
    }
}

impl Tagged for Credit {
    fn tag(&self) -> steam_track::Tag {
        steam_track::Tag(0)
    }
}

impl std::fmt::Display for Credit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "credit {}", self.0)
    }
}

impl SimObject for Credit {}
