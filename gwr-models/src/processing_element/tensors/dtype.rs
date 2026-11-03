// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(
    ValueEnum,
    Copy,
    Clone,
    Debug,
    Default,
    Deserialize,
    Serialize,
    PartialEq,
    PartialOrd,
    Eq,
    Ord,
    Hash,
)]
#[serde(rename_all = "lowercase")]
pub enum DataType {
    #[default]
    Fp32,
    Bf16,
    Fp16,
    Fp8,
    Fp4,
    Int64,
    Int32,
    Int16,
    Int8,
    Int4,
    Uint64,
    Uint32,
    Uint16,
    Uint8,
    Bool,
}

impl DataType {
    /// All supported storage datatypes (not a precision ranking).
    pub const ALL: [Self; 15] = [
        Self::Fp32,
        Self::Bf16,
        Self::Fp16,
        Self::Fp8,
        Self::Fp4,
        Self::Int64,
        Self::Int32,
        Self::Int16,
        Self::Int8,
        Self::Int4,
        Self::Uint64,
        Self::Uint32,
        Self::Uint16,
        Self::Uint8,
        Self::Bool,
    ];

    /// Return the number of bits required
    #[must_use]
    pub fn num_bits(&self) -> usize {
        match self {
            DataType::Fp32 => 32,
            DataType::Fp16 => 16,
            DataType::Bf16 => 16,
            DataType::Fp8 => 8,
            DataType::Fp4 => 4,
            DataType::Int64 => 64,
            DataType::Int32 => 32,
            DataType::Int16 => 16,
            DataType::Int8 => 8,
            DataType::Int4 => 4,
            DataType::Uint64 => 64,
            DataType::Uint32 => 32,
            DataType::Uint16 => 16,
            DataType::Uint8 => 8,
            DataType::Bool => 1,
        }
    }
}
