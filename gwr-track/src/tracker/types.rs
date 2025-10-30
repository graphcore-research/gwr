// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::fmt;

use num_derive::{FromPrimitive, ToPrimitive};

// Generic packet request types
#[derive(Copy, Clone, Debug, Default, FromPrimitive, PartialEq, ToPrimitive)]
#[allow(missing_docs)]
pub enum ReqType {
    #[default]
    Read, //
    Write,
    WriteNonPosted,
    Value,
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
            ReqType::Value => {
                write!(f, "Value")
            }
        }
    }
}
