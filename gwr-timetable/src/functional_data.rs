// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! CLI inputs and independent reference outputs for functional execution.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gwr_models::memory::{FunctionalMemory, FunctionalValue};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_platform::Platform;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Values {
    address: u64,
    #[serde(default)]
    dtype: DataType,
    #[serde(default)]
    values: Option<Vec<yaml_serde::Value>>,
    #[serde(default)]
    bits: Option<Vec<u64>>,
    #[serde(default)]
    file: Option<PathBuf>,
    #[serde(default)]
    count: Option<usize>,
}

impl Values {
    fn load(&self, directory: &Path) -> super::Result<LoadedValues> {
        if self.count.is_some() && self.file.is_none() {
            return Err("functional count is only valid with file".into());
        }
        let (values, exact) = match (&self.values, &self.bits, &self.file) {
            (Some(values), None, None) => (
                values
                    .iter()
                    .map(|value| self.encode(value))
                    .collect::<super::Result<Vec<_>>>()?,
                false,
            ),
            (None, Some(bits), None) => (
                bits.iter()
                    .map(|bits| Ok(FunctionalValue::from_bits(self.dtype, *bits)?))
                    .collect::<super::Result<Vec<_>>>()?,
                true,
            ),
            (None, None, Some(file)) => {
                let path = directory.join(file);
                let bytes = std::fs::read(&path).map_err(|error| {
                    format!("cannot read functional file {}: {error}", path.display())
                })?;
                let width = self.dtype.num_bits();
                let available = bytes
                    .len()
                    .checked_mul(8)
                    .ok_or("functional file size overflows")?;
                let count = match self.count {
                    Some(count) => count,
                    None if width < 8 => return Err("packed functional files require count".into()),
                    None => available / width,
                };
                if count
                    .checked_mul(width)
                    .ok_or("functional bit count overflows")?
                    .div_ceil(8)
                    != bytes.len()
                {
                    return Err(if self.dtype == DataType::Fp32 {
                        "functional FP32 file length must be a multiple of four and match count"
                    } else {
                        "functional file length must match dtype and count"
                    }
                    .into());
                }
                if self.dtype == DataType::Fp32 {
                    return Ok(LoadedValues::Fp32(
                        bytes
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|bytes| f32::from_le_bytes(*bytes))
                            .collect(),
                    ));
                }
                let mask = u64::MAX >> (64 - width);
                (
                    (0..count)
                        .map(|index| {
                            let start = index * width;
                            let offset = start % 8;
                            let bits = bytes[start / 8..][..(offset + width).div_ceil(8)]
                                .iter()
                                .enumerate()
                                .fold(0u64, |bits, (i, byte)| bits | (u64::from(*byte) << (i * 8)));
                            Ok(FunctionalValue::from_bits(
                                self.dtype,
                                (bits >> offset) & mask,
                            )?)
                        })
                        .collect::<super::Result<Vec<_>>>()?,
                    false,
                )
            }
            _ => {
                return Err(
                    "functional data range requires exactly one of values, bits or file".into(),
                );
            }
        };
        if self.dtype == DataType::Fp32 && !exact {
            Ok(LoadedValues::Fp32(
                values
                    .iter()
                    .map(|value| value.as_f32())
                    .collect::<Result<_, _>>()?,
            ))
        } else {
            Ok(LoadedValues::Typed {
                dtype: self.dtype,
                values,
                exact,
            })
        }
    }

    fn encode(&self, value: &yaml_serde::Value) -> super::Result<FunctionalValue> {
        if is_float(self.dtype) {
            Ok(FunctionalValue::from_f32(
                self.dtype,
                value
                    .as_f64()
                    .ok_or("functional floating value must be numeric")? as f32,
            )?)
        } else if self.dtype == DataType::Bool {
            Ok(FunctionalValue::from_bool(
                value
                    .as_bool()
                    .ok_or("functional Bool value must be true or false")?,
            ))
        } else if matches!(
            self.dtype,
            DataType::Int64 | DataType::Int32 | DataType::Int16 | DataType::Int8 | DataType::Int4
        ) {
            Ok(FunctionalValue::from_i64(
                self.dtype,
                value
                    .as_i64()
                    .ok_or("functional signed value must be an exact integer")?,
            )?)
        } else {
            Ok(FunctionalValue::from_u64(
                self.dtype,
                value
                    .as_u64()
                    .ok_or("functional unsigned value must be an exact nonnegative integer")?,
            )?)
        }
    }
}

fn is_float(dtype: DataType) -> bool {
    matches!(
        dtype,
        DataType::Fp32 | DataType::Bf16 | DataType::Fp16 | DataType::Fp8 | DataType::Fp4
    )
}

enum LoadedValues {
    // Keep large existing FP32 fixtures on the bulk loading/checking path.
    Fp32(Vec<f32>),
    Typed {
        dtype: DataType,
        values: Vec<FunctionalValue>,
        exact: bool,
    },
}

impl LoadedValues {
    fn len(&self) -> usize {
        match self {
            Self::Fp32(values) => values.len(),
            Self::Typed { values, .. } => values.len(),
        }
    }

    fn store(&self, memory: &FunctionalMemory, address: u64) -> super::Result<()> {
        match self {
            Self::Fp32(values) => memory.write_f32(address, values)?,
            Self::Typed { dtype, values, .. } => memory.write_values(address, *dtype, values)?,
        }
        Ok(())
    }

    fn check(
        &self,
        memory: &FunctionalMemory,
        address: u64,
        atol: f32,
        rtol: f32,
    ) -> super::Result<f64> {
        match self {
            Self::Fp32(values) => Ok(memory
                .check_f32(address, values, atol, rtol)?
                .max_absolute_error),
            Self::Typed {
                dtype,
                values,
                exact,
            } if *exact || !is_float(*dtype) => {
                memory.check_values(address, *dtype, values)?;
                Ok(0.)
            }
            Self::Typed { dtype, values, .. } => {
                let actual = memory.read_values(address, *dtype, values.len())?;
                let mut max_error = 0.0f64;
                for (index, (actual, expected)) in actual.iter().zip(values).enumerate() {
                    let actual = f64::from(actual.as_f32()?);
                    let expected = f64::from(expected.as_f32()?);
                    let error = (actual - expected).abs();
                    if !actual.is_finite()
                        || !expected.is_finite()
                        || error > f64::from(atol) + f64::from(rtol) * expected.abs()
                    {
                        return Err(format!("functional mismatch at element {index}: actual {actual}, expected {expected}").into());
                    }
                    max_error = max_error.max(error);
                }
                Ok(max_error)
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    inputs: Vec<Values>,
    checks: Vec<Values>,
    #[serde(default = "default_atol")]
    atol: f32,
    #[serde(default = "default_rtol")]
    rtol: f32,
}

fn default_atol() -> f32 {
    1e-5
}

fn default_rtol() -> f32 {
    1e-4
}

pub(super) struct FunctionalData {
    memory: Rc<FunctionalMemory>,
    checks: Vec<(u64, LoadedValues)>,
    atol: f32,
    rtol: f32,
}

impl FunctionalData {
    pub(super) fn load(path: &Path, platform: &Platform) -> super::Result<Self> {
        let fixture: Fixture = yaml_serde::from_str(&std::fs::read_to_string(path)?)?;
        let directory = path.parent().unwrap_or(Path::new("."));
        let checks = fixture
            .checks
            .iter()
            .map(|check| Ok((check.address, check.load(directory)?)))
            .collect::<super::Result<Vec<_>>>()?;
        if checks.is_empty() || checks.iter().any(|(_, values)| values.len() == 0) {
            return Err("functional data requires at least one nonempty reference check".into());
        }
        if [fixture.atol, fixture.rtol]
            .into_iter()
            .any(|value| !value.is_finite() || value < 0.0)
        {
            return Err("functional tolerances must be finite and nonnegative".into());
        }
        let memory = Rc::new(FunctionalMemory::default());
        for input in &fixture.inputs {
            input.load(directory)?.store(&memory, input.address)?;
        }
        for name in platform.pe_names() {
            platform.pe(&name)?.enable_functional(memory.clone())?;
        }
        Ok(Self {
            memory,
            checks,
            atol: fixture.atol,
            rtol: fixture.rtol,
        })
    }

    pub(super) fn check(&self) -> super::Result<()> {
        let mut elements = 0usize;
        let mut max_absolute_error = 0.0f64;
        for (address, values) in &self.checks {
            let error = values.check(&self.memory, *address, self.atol, self.rtol)?;
            elements = elements
                .checked_add(values.len())
                .ok_or("functional check count overflows")?;
            max_absolute_error = max_absolute_error.max(error);
        }
        println!(
            "Functional checks passed: {elements} elements, max absolute error {max_absolute_error}"
        );
        Ok(())
    }
}
