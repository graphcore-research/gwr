// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Typed compute capabilities and staged execution on shared PE execution
//! units.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Formatter;
use std::marker::PhantomData;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;
use serde::de::{Error, MapAccess};
use serde::{Deserialize, Deserializer, Serialize};

use super::tensors::dtype::DataType;

mod lower;
mod scheduler;
mod work;
pub(super) use scheduler::{ComputeReservation, ComputeScheduler};
pub(super) use work::{CapabilityKey, ComputeBatch, ExecutionPlan, TypedWork, WorkStage};
pub use work::{ComputeWork, MatrixWorkShape};

/// Pure, validated compute model; mutable arbitration belongs to the scheduler.
#[derive(Debug)]
pub struct ComputeModel {
    names: Vec<String>,
    matrix_accumulator_capacities: Vec<Option<u64>>,
    capabilities: BTreeMap<String, Vec<CapabilityMatch>>,
}

impl ComputeModel {
    /// Validate and expand hardware declarations once, with stable name
    /// ordering.
    pub fn new(
        config: &ComputeConfig,
        classes: &BTreeMap<String, OperationClass>,
    ) -> Result<Self, SimError> {
        if config.execution_units.is_empty() {
            return sim_error!("execution units must not be empty");
        }
        validate_operation_classes(classes)?;
        let mut model = Self {
            names: Vec::new(),
            matrix_accumulator_capacities: Vec::new(),
            capabilities: BTreeMap::new(),
        };
        for (index, (name, unit)) in config.execution_units.iter().enumerate() {
            model.register_execution_unit(ExecutionUnitId(index), name, unit, classes)?;
        }
        Ok(model)
    }

    fn register_execution_unit(
        &mut self,
        id: ExecutionUnitId,
        name: &str,
        unit: &ExecutionUnitConfig,
        classes: &BTreeMap<String, OperationClass>,
    ) -> Result<(), SimError> {
        if name.trim().is_empty() || unit.capabilities.is_empty() {
            return sim_error!("execution unit '{name}' must have a name and capabilities");
        }
        validate_accumulator_capacity(unit, classes)?;
        self.names.push(name.to_owned());
        self.matrix_accumulator_capacities
            .push(unit.matrix_accumulator_capacity_bytes);
        for (entry, capability) in unit.capabilities.iter().enumerate() {
            let class = classes.get(&capability.class).ok_or_else(|| {
                SimError(format!(
                    "execution unit '{name}' capability {entry}: unknown operation class '{}'",
                    capability.class
                ))
            })?;
            capability.validate(class).map_err(|e| {
                SimError(format!("execution unit '{name}' capability {entry}: {e}"))
            })?;
            self.register_capability(id, name, capability, class)?;
        }
        Ok(())
    }

    fn register_capability(
        &mut self,
        execution_unit: ExecutionUnitId,
        name: &str,
        capability: &CapabilityConfig,
        class: &OperationClass,
    ) -> Result<(), SimError> {
        for op in &class.operations {
            let matches = self.capabilities.entry(op.clone()).or_default();
            let signatures: Vec<Option<Vec<DataType>>> = match &capability.input_dtypes {
                Some(combinations) => combinations.iter().cloned().map(Some).collect(),
                None => vec![None],
            };
            for inputs in signatures {
                if matches.iter().any(|prior| {
                    prior.execution_unit == execution_unit
                        && prior.arity == class.arity
                        && prior.inputs == inputs
                        && prior.kind.same_precision(capability.kind)
                }) {
                    return sim_error!(
                        "execution unit '{name}': overlapping capabilities for {op}"
                    );
                }
                matches.push(CapabilityMatch {
                    kind: capability.kind,
                    execution_unit,
                    rate: capability.ops_per_tick,
                    inputs,
                    arity: class.arity,
                    flops_per_op: class.flops_per_op,
                });
            }
        }
        Ok(())
    }

    /// Validate capability support, service geometry and accumulator capacity
    /// without reserving resources or exposing the execution plan.
    pub fn validate_work(&self, work: &ComputeWork) -> Result<(), SimError> {
        self.lower(work).map(drop)
    }

    /// Execution unit names in stable execution order.
    #[must_use]
    pub(super) fn execution_unit_names(&self) -> &[String] {
        &self.names
    }

    fn select(&self, key: &CapabilityKey) -> Vec<ComputeCandidate> {
        let Some(matches) = self.capabilities.get(key.op()) else {
            return vec![];
        };
        let eligible = matches.iter().filter(|candidate| {
            candidate.kind.matches(key) && candidate.arity == key.inputs().len()
        });
        let has_explicit_match = eligible
            .clone()
            .any(|candidate| candidate.inputs.as_deref() == Some(key.inputs()));
        let inputs = if has_explicit_match {
            Some(key.inputs())
        } else {
            None
        };
        eligible
            .filter(|candidate| candidate.inputs.as_deref() == inputs)
            .map(|candidate| ComputeCandidate {
                execution_unit: candidate.execution_unit,
                rate: candidate.rate,
                flops_per_op: candidate.flops_per_op,
                matrix_service_tile: candidate.kind.matrix_service_tile(),
                matrix_accumulator_capacity_bytes: self.matrix_accumulator_capacities
                    [candidate.execution_unit.0],
            })
            .collect()
    }
}

fn validate_operation_classes(classes: &BTreeMap<String, OperationClass>) -> Result<(), SimError> {
    for (name, class) in classes {
        if name.trim().is_empty()
            || class.arity == 0
            || class.operations.is_empty()
            || class.operations.iter().collect::<BTreeSet<_>>().len() != class.operations.len()
        {
            return sim_error!("invalid operation class '{name}'");
        }
        if class
            .operations
            .iter()
            .any(|op| !is_lowercase_operation(op))
        {
            return sim_error!("operation names in class '{name}' must be lowercase");
        }
    }
    Ok(())
}

fn validate_accumulator_capacity(
    unit: &ExecutionUnitConfig,
    classes: &BTreeMap<String, OperationClass>,
) -> Result<(), SimError> {
    let Some(capacity) = unit.matrix_accumulator_capacity_bytes else {
        return Ok(());
    };
    if capacity == 0 {
        return sim_error!("matrix accumulator capacity must be positive");
    }
    let mut has_matrix = false;
    for capability in &unit.capabilities {
        let class = classes
            .get(&capability.class)
            .ok_or_else(|| SimError(format!("unknown operation class '{}'", capability.class)))?;
        if class.operations.iter().any(|op| op == "matrix_mac") {
            has_matrix = true;
            if class.arity != 2
                || class.operations != ["matrix_mac"]
                || capability.input_dtypes.is_none()
            {
                return sim_error!(
                    "matrix accumulator capacity requires exact matrix declarations"
                );
            }
        }
    }
    if !has_matrix {
        return sim_error!("matrix accumulator capacity requires a matrix declaration");
    }
    Ok(())
}

/// PE compute hardware.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeConfig {
    /// Named independent execution units.
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub execution_units: BTreeMap<String, ExecutionUnitConfig>,
}

/// One execution unit: all its typed capabilities share a single batch slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionUnitConfig {
    /// Logical accumulator capacity local to this execution unit; independent
    /// of SRAM storage and service padding. Omitted preserves unconstrained
    /// admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_accumulator_capacity_bytes: Option<u64>,
    /// Compact capability declarations, preserved by YAML serialization.
    pub capabilities: Vec<CapabilityConfig>,
}

/// A compact declaration of typed operations sharing one throughput rate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "CapabilityConfigRepr", into = "CapabilityConfigRepr")]
pub struct CapabilityConfig {
    /// Platform-wide operation class.
    pub class: String,
    /// Exact ordered input datatype combinations. Omit to cover
    /// combinations not explicitly declared for this class.
    pub input_dtypes: Option<Vec<Vec<DataType>>>,
    /// Logical operations per tick for the whole execution unit.
    pub ops_per_tick: f64,
    /// Matrix declarations carry a required accumulator precision.
    pub kind: CapabilityKind,
}

impl From<CapabilityConfigRepr> for CapabilityConfig {
    fn from(value: CapabilityConfigRepr) -> Self {
        match value {
            CapabilityConfigRepr::Matrix {
                class,
                input_dtypes,
                ops_per_tick,
                accumulator,
                matrix_service_tile,
            } => Self {
                class,
                input_dtypes,
                ops_per_tick,
                kind: CapabilityKind::Matrix {
                    accumulator,
                    matrix_service_tile,
                },
            },
            CapabilityConfigRepr::Scalar {
                class,
                input_dtypes,
                ops_per_tick,
            } => Self {
                class,
                input_dtypes,
                ops_per_tick,
                kind: CapabilityKind::Scalar,
            },
        }
    }
}

impl CapabilityConfig {
    fn validate(&self, class: &OperationClass) -> Result<(), SimError> {
        if !self.ops_per_tick.is_finite() || self.ops_per_tick <= 0.0 {
            return sim_error!("compute throughput must be finite and greater than zero");
        }
        if let Some(pairs) = &self.input_dtypes
            && (pairs.is_empty()
                || pairs.iter().any(|pair| pair.len() != class.arity)
                || pairs.iter().collect::<BTreeSet<_>>().len() != pairs.len())
        {
            return sim_error!(
                "input_dtypes must contain distinct combinations of class arity {}",
                class.arity
            );
        }
        let matrix = class.operations.iter().any(|op| op == "matrix_mac");
        if matrix
            && (class.arity != 2
                || class.operations != ["matrix_mac"]
                || !matches!(self.kind, CapabilityKind::Matrix { .. }))
        {
            return sim_error!(
                "matrix_mac capability requires a two-input matrix class and accumulator"
            );
        }
        if !matrix && matches!(self.kind, CapabilityKind::Matrix { .. }) {
            return sim_error!("accumulator is only supported for matrix_mac capabilities");
        }
        if let Some(tile) = self.kind.matrix_service_tile() {
            tile.volume()?;
            if class.arity != 2 || class.operations != ["matrix_mac"] || self.input_dtypes.is_none()
            {
                return sim_error!(
                    "matrix service tile requires exact matrix_mac class/operands/accumulator"
                );
            }
        }
        Ok(())
    }
}

// Keep platform YAML flat while representing matrix settings together in Rust.
// An accumulator selects the matrix declaration; scalar declarations have no
// matrix-only fields.
#[derive(Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum CapabilityConfigRepr {
    Matrix {
        class: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_dtypes: Option<Vec<Vec<DataType>>>,
        ops_per_tick: f64,
        accumulator: DataType,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        matrix_service_tile: Option<MatrixServiceTile>,
    },
    Scalar {
        class: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_dtypes: Option<Vec<Vec<DataType>>>,
        ops_per_tick: f64,
    },
}

impl From<CapabilityConfig> for CapabilityConfigRepr {
    fn from(value: CapabilityConfig) -> Self {
        let CapabilityConfig {
            class,
            input_dtypes,
            ops_per_tick,
            kind,
        } = value;
        match kind {
            CapabilityKind::Matrix {
                accumulator,
                matrix_service_tile,
            } => Self::Matrix {
                class,
                input_dtypes,
                ops_per_tick,
                accumulator,
                matrix_service_tile,
            },
            CapabilityKind::Scalar => Self::Scalar {
                class,
                input_dtypes,
                ops_per_tick,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationClass {
    pub operations: Vec<String>,
    pub arity: usize,
    #[serde(default = "one_flop")]
    pub flops_per_op: usize,
}

#[derive(Debug)]
struct CapabilityMatch {
    kind: CapabilityKind,
    execution_unit: ExecutionUnitId,
    rate: f64,
    inputs: Option<Vec<DataType>>,
    arity: usize,
    flops_per_op: usize,
}

/// Matrix-only settings cannot occur without an accumulator datatype.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CapabilityKind {
    Scalar,
    Matrix {
        accumulator: DataType,
        matrix_service_tile: Option<MatrixServiceTile>,
    },
}

impl CapabilityKind {
    fn same_precision(self, other: Self) -> bool {
        match (self, other) {
            (Self::Scalar, Self::Scalar) => true,
            (
                Self::Matrix {
                    accumulator: lhs, ..
                },
                Self::Matrix {
                    accumulator: rhs, ..
                },
            ) => lhs == rhs,
            _ => false,
        }
    }

    fn matches(self, key: &CapabilityKey) -> bool {
        match (self, key) {
            (Self::Scalar, CapabilityKey::Operation { .. }) => true,
            (
                Self::Matrix { accumulator, .. },
                CapabilityKey::MatrixMac {
                    accumulator: required,
                    ..
                },
            ) => accumulator == *required,
            _ => false,
        }
    }

    fn matrix_service_tile(self) -> Option<MatrixServiceTile> {
        match self {
            Self::Scalar => None,
            Self::Matrix {
                matrix_service_tile,
                ..
            } => matrix_service_tile,
        }
    }
}

/// Private selected service option; logical flop cost is independent of policy.
#[derive(Clone, Copy, Debug)]
struct ComputeCandidate {
    matrix_accumulator_capacity_bytes: Option<u64>,
    execution_unit: ExecutionUnitId,
    rate: f64,
    flops_per_op: usize,
    matrix_service_tile: Option<MatrixServiceTile>,
}

/// Compute block dimensions for service timing, independent of physical
/// traffic.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixServiceTile {
    pub m: usize,
    pub n: usize,
    pub k: usize,
}

impl MatrixServiceTile {
    pub(crate) fn service_macs(self, shape: MatrixWorkShape) -> Result<usize, SimError> {
        self.volume()?;
        let rounded = |size: usize, tile: usize| -> Result<usize, SimError> {
            let groups = (size / tile)
                .checked_add(usize::from(!size.is_multiple_of(tile)))
                .ok_or_else(|| SimError("matrix service shape overflows".into()))?;
            groups
                .checked_mul(tile)
                .ok_or_else(|| SimError("matrix service shape overflows".into()))
        };
        MatrixWorkShape {
            batches: shape.batches,
            m: rounded(shape.m, self.m)?,
            n: rounded(shape.n, self.n)?,
            k: rounded(shape.k, self.k)?,
        }
        .logical_macs()
    }

    fn volume(self) -> Result<usize, SimError> {
        MatrixWorkShape {
            batches: 1,
            m: self.m,
            n: self.n,
            k: self.k,
        }
        .logical_macs()
    }
}

/// Stable execution unit index in a validated model.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct ExecutionUnitId(pub(super) usize);

fn one_flop() -> usize {
    1
}

pub(crate) fn is_lowercase_operation(name: &str) -> bool {
    !name.trim().is_empty() && name == name.to_lowercase()
}

// Reject duplicate map keys instead of allowing serde maps to overwrite them.
fn deserialize_unique_map<'de, D, V>(deserializer: D) -> Result<BTreeMap<String, V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct Visitor<V>(PhantomData<V>);

    impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<V> {
        type Value = BTreeMap<String, V>;

        fn expecting(&self, f: &mut Formatter) -> fmt::Result {
            f.write_str("a map with unique names")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut values = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, V>()? {
                if values.insert(key.clone(), value).is_some() {
                    return Err(Error::custom(format!("duplicate key '{key}'")));
                }
            }
            Ok(values)
        }
    }
    deserializer.deserialize_map(Visitor(PhantomData))
}

#[cfg(test)]
mod matrix_accumulator_capacity_tests;
#[cfg(test)]
mod matrix_accumulator_tests;
#[cfg(test)]
mod matrix_service_shape_tests;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod native_matrix_tests;
#[cfg(test)]
mod operation_class_tests;
#[cfg(test)]
mod scheduler_tests;
