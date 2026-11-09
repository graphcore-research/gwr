// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::processing_element::tensors::dtype::DataType;

#[test]
fn predicates_have_one_bit_boolean_result_storage() {
    let dtype: DataType = yaml_serde::from_str("bool").unwrap();
    assert_eq!(dtype.num_bits(), 1);
}

#[test]
fn unsigned_bitwise_operands_have_storage_dtype() {
    let dtype: DataType = yaml_serde::from_str("uint32").unwrap();
    assert_eq!(dtype.num_bits(), 32);
}
