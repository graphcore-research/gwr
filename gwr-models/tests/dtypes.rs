// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::processing_element::tensors::dtype::DataType;
use yaml_serde::from_str;

#[test]
fn bool_dtype_parses_with_one_bit_per_element() {
    let dtype: DataType = from_str("bool").unwrap();
    assert_eq!(dtype.num_bits(), 1);
}

#[test]
fn uint32_dtype_parses_with_32_bits_per_element() {
    let dtype: DataType = from_str("uint32").unwrap();
    assert_eq!(dtype.num_bits(), 32);
}
