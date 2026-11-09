// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Tensor types and views used by processing elements.

pub mod dtype;
mod floor_sum;
mod tensor;
mod tensor_view;

pub use tensor::{HasShape, Shape, Tensor, shape_string};
pub use tensor_view::{TensorView, TensorViewLayout, TensorViewStride};
