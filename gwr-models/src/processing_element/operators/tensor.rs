// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fmt::{self, Display};

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};

use super::dtype::DataType;

pub trait HasShape {
    #[must_use]
    fn num_dims(&self) -> usize;

    #[must_use]
    fn num_elements(&self) -> usize;

    /// Return a dimension after left-padding the shape with ones.
    #[must_use]
    fn get_dim(&self, total_dims: usize, index: usize) -> usize;

    #[must_use]
    fn shape(&self) -> &Shape;
}

impl<T> HasShape for &T
where
    T: HasShape,
{
    fn num_dims(&self) -> usize {
        (*self).num_dims()
    }

    fn num_elements(&self) -> usize {
        (*self).num_elements()
    }

    fn get_dim(&self, total_dims: usize, index: usize) -> usize {
        (*self).get_dim(total_dims, index)
    }

    fn shape(&self) -> &Shape {
        (*self).shape()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Shape {
    dims: Vec<usize>,
}

impl Shape {
    pub fn new(dims: &[usize]) -> Result<Self, SimError> {
        if let Some(dimension) = dims.iter().position(|extent| *extent == 0) {
            return sim_error!("Shape {dims:?} has zero size in dimension {dimension}");
        }

        checked_product(
            dims.iter().copied(),
            &format!("Shape {dims:?} element count"),
        )?;
        Ok(Self {
            dims: dims.to_vec(),
        })
    }

    #[must_use]
    pub fn dims(&self) -> &[usize] {
        &self.dims
    }
}

impl Display for Shape {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", shape_string(&self.dims))
    }
}

impl HasShape for Shape {
    fn num_dims(&self) -> usize {
        self.dims.len()
    }

    fn num_elements(&self) -> usize {
        self.dims.iter().product()
    }

    fn get_dim(&self, total_dims: usize, index: usize) -> usize {
        let dim_index = total_dims - index;
        let rank = self.num_dims();
        if dim_index <= rank {
            self.dims[rank - dim_index]
        } else {
            1
        }
    }

    fn shape(&self) -> &Shape {
        self
    }
}

#[derive(Clone, Debug)]
pub struct Tensor {
    pub(super) id: Option<String>,
    pub(super) dtype: DataType,
    pub(super) shape: Shape,
    pub(super) addr: u64,
    pub(super) num_bytes: usize,
}

impl Tensor {
    pub fn new(dims: &[usize], dtype: &DataType, addr: u64) -> Result<Self, SimError> {
        Self::from_shape(Shape::new(dims)?, dtype, addr)
    }

    pub fn from_shape(shape: Shape, dtype: &DataType, addr: u64) -> Result<Self, SimError> {
        let num_bytes = checked_num_bytes(shape.num_elements(), dtype, "Tensor")?;
        validate_address_range(addr, num_bytes)?;
        Ok(Self {
            id: None,
            shape,
            dtype: *dtype,
            addr,
            num_bytes,
        })
    }

    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn set_id(&mut self, id: impl Into<String>) {
        self.id = Some(id.into());
    }

    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Return the packed size of the complete tensor.
    #[must_use]
    pub fn num_bytes(&self) -> usize {
        self.num_bytes
    }

    #[must_use]
    pub fn dtype(&self) -> &DataType {
        &self.dtype
    }

    #[must_use]
    pub fn addr(&self) -> u64 {
        self.addr
    }

    pub fn set_addr(&mut self, addr: u64) -> SimResult {
        validate_address_range(addr, self.num_bytes)?;
        self.addr = addr;
        Ok(())
    }
}

impl HasShape for Tensor {
    fn num_dims(&self) -> usize {
        self.shape.num_dims()
    }

    fn num_elements(&self) -> usize {
        self.shape.num_elements()
    }

    fn get_dim(&self, total_dims: usize, index: usize) -> usize {
        self.shape.get_dim(total_dims, index)
    }

    fn shape(&self) -> &Shape {
        &self.shape
    }
}

#[must_use]
pub fn shape_string(dims: &[usize]) -> String {
    dims.iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join("×")
}

pub(super) fn checked_num_bytes(
    num_elements: usize,
    dtype: &DataType,
    description: &str,
) -> Result<usize, SimError> {
    let num_bits = num_elements as u128 * dtype.num_bits() as u128;
    let num_bytes = num_bits.div_ceil(8);

    usize::try_from(num_bytes)
        .map_err(|_| SimError(format!("{description} storage size overflows")))
}

fn checked_product(
    values: impl IntoIterator<Item = usize>,
    description: &str,
) -> Result<usize, SimError> {
    values.into_iter().try_fold(1usize, |product, value| {
        product
            .checked_mul(value)
            .ok_or_else(|| SimError(format!("{description} overflows")))
    })
}

fn validate_address_range(addr: u64, num_bytes: usize) -> SimResult {
    let num_bytes = u64::try_from(num_bytes)
        .map_err(|error| SimError(format!("Tensor storage size: {error}")))?;
    let last_address = num_bytes
        .checked_sub(1)
        .and_then(|last_offset| addr.checked_add(last_offset));
    if last_address.is_none() {
        return sim_error!(
            "Tensor at 0x{addr:x} with size {num_bytes} bytes exceeds the physical address space"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_shapes_and_storage_sizes() {
        assert!(Shape::new(&[usize::MAX, 2]).is_err());
        assert!(Shape::new(&[4, 0]).is_err());
        assert!(Tensor::new(&[usize::MAX / 2], &DataType::Int64, 0).is_err());
    }

    #[test]
    fn preserves_scalar_shapes() {
        let scalar = Shape::new(&[]).unwrap();
        assert_eq!(scalar.num_elements(), 1);
        assert_eq!(Tensor::new(&[], &DataType::Fp32, 0).unwrap().num_bytes(), 4);
    }

    #[test]
    fn validates_tensor_addresses() {
        assert!(Tensor::new(&[2], &DataType::Int8, u64::MAX).is_err());
        assert!(Tensor::new(&[1], &DataType::Int8, u64::MAX).is_ok());

        let mut tensor = Tensor::new(&[2], &DataType::Int8, 0).unwrap();
        assert!(tensor.set_addr(u64::MAX).is_err());
        assert_eq!(tensor.addr(), 0);
    }
}
