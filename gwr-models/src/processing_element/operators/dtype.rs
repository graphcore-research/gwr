// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use serde::{Deserialize, Serialize};

#[derive(clap::ValueEnum, Clone, Debug, Default, Deserialize, Serialize, PartialEq, PartialOrd)]
#[serde(rename_all = "lowercase")]
pub enum DataType {
    #[default]
    Fp32,
    Bf16,
    Fp16,
    Fp8,
    Fp4,
    Int32,
    Int16,
    Int8,
    Int4,
}

impl DataType {
    #[must_use]
    /// Return the number of bits required
    pub fn num_bits(&self) -> usize {
        match self {
            DataType::Fp32 => 32,
            DataType::Fp16 => 16,
            DataType::Bf16 => 16,
            DataType::Fp8 => 8,
            DataType::Fp4 => 4,
            DataType::Int32 => 32,
            DataType::Int16 => 16,
            DataType::Int8 => 8,
            DataType::Int4 => 4,
        }
    }
}
