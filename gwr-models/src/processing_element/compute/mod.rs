// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Typed compute capabilities and staged execution on shared PE engines.

use std::collections::{BTreeMap, BTreeSet};

use gwr_engine::sim_error;
use gwr_engine::types::SimError;
use serde::{Deserialize, Serialize};

use super::tensors::dtype::DataType;

mod counters;
mod lower;
pub use counters::{
    ComputeAction, ComputeCounterSnapshot, ComputeCounters, NativeActivityReport,
    NativeMemoryPayload, NativePeActivity,
};
mod scheduler;
mod work;
pub use scheduler::{
    AccumulatorLease, AcquireAccumulator, AcquireBatch, ComputeReservation, ComputeScheduler,
    RetainedComputeReservation,
};
pub use work::{
    CapabilityKey, ComputeWork, EngineBatch, ExecutionPlan, MatrixWorkShape, TypedWork, WorkStage,
};

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
    #[expect(
        clippy::too_many_lines,
        reason = "Validate every engine before exposing the compute model"
    )]
    pub fn new(
        config: &ComputeConfig,
        classes: &BTreeMap<String, OperationClass>,
    ) -> Result<Self, SimError> {
        if config.engines.is_empty() {
            return sim_error!("compute engines must not be empty");
        }
        let mut names = Vec::new();
        let mut matrix_accumulator_capacities = Vec::new();
        let mut capabilities: BTreeMap<String, Vec<CapabilityMatch>> = BTreeMap::new();
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
        for (index, (name, engine)) in config.engines.iter().enumerate() {
            if name.trim().is_empty() || engine.capabilities.is_empty() {
                return sim_error!("engine '{name}' must have a name and capabilities");
            }
            if let Some(capacity) = engine.matrix_accumulator_capacity_bytes {
                if capacity == 0 {
                    return sim_error!("matrix accumulator capacity must be positive");
                }
                let mut has_matrix = false;
                for capability in &engine.capabilities {
                    let class = classes.get(&capability.class).ok_or_else(|| {
                        SimError(format!("unknown operation class '{}'", capability.class))
                    })?;
                    if class.operations.iter().any(|op| op == "matrix_mac") {
                        has_matrix = true;
                        if class.arity != 2
                            || class.operations != ["matrix_mac"]
                            || capability.input_dtypes.is_none()
                            || capability.execution.as_ref().is_none_or(|mode| {
                                mode.instruction.trim().is_empty()
                                    || mode.sparsity != Sparsity::Dense
                            })
                        {
                            return sim_error!(
                                "matrix accumulator capacity requires exact dense matrix declarations"
                            );
                        }
                    }
                }
                if !has_matrix {
                    return sim_error!("matrix accumulator capacity requires a matrix declaration");
                }
            }
            names.push(name.clone());
            matrix_accumulator_capacities.push(engine.matrix_accumulator_capacity_bytes);
            for (entry, capability) in engine.capabilities.iter().enumerate() {
                let class = classes.get(&capability.class).ok_or_else(|| {
                    SimError(format!(
                        "engine '{name}' capability {entry}: unknown operation class '{}'",
                        capability.class
                    ))
                })?;
                capability
                    .validate(class)
                    .map_err(|e| SimError(format!("engine '{name}' capability {entry}: {e}")))?;
                for op in &class.operations {
                    let matches = capabilities.entry(op.clone()).or_default();
                    let signatures: Vec<Option<Vec<DataType>>> = match &capability.input_dtypes {
                        Some(combinations) => combinations.iter().cloned().map(Some).collect(),
                        None => vec![None],
                    };
                    for inputs in signatures {
                        if matches.iter().any(|prior| {
                            prior.engine == EngineId(index)
                                && prior.arity == class.arity
                                && prior.inputs == inputs
                                && prior.execution == capability.execution
                        }) {
                            return sim_error!(
                                "engine '{name}': overlapping capabilities for {op}"
                            );
                        }
                        matches.push(CapabilityMatch {
                            execution: capability.execution.clone(),
                            matrix_service_tile: capability.matrix_service_tile,
                            engine: EngineId(index),
                            rate: capability.ops_per_tick,
                            inputs,
                            arity: class.arity,
                            flops_per_op: class.flops_per_op,
                        });
                    }
                }
            }
        }
        Ok(Self {
            names,
            matrix_accumulator_capacities,
            capabilities,
        })
    }

    /// Engine names in stable execution order.
    #[must_use]
    pub fn engine_names(&self) -> &[String] {
        &self.names
    }

    /// Any matching candidate policy requires canonical geometry, including
    /// plain alternatives.
    pub(crate) fn requires_matrix_shape(&self, key: &CapabilityKey) -> bool {
        self.select(key).iter().any(|candidate| {
            candidate.matrix_service_tile.is_some()
                || (key.op == "matrix_mac" && candidate.matrix_accumulator_capacity_bytes.is_some())
        })
    }

    /// Whether the exact ordered operand tuple and execution mode were
    /// declared. Opted-in block pipelines require this qualification rather
    /// than a wildcard rate.
    pub(crate) fn has_explicit_capability(&self, key: &CapabilityKey) -> bool {
        self.capabilities.get(&key.op).is_some_and(|matches| {
            matches.iter().any(|candidate| {
                candidate.execution == key.execution
                    && candidate.arity == key.inputs.len()
                    && candidate.inputs.as_ref() == Some(&key.inputs)
            })
        })
    }

    pub(crate) fn select(&self, key: &CapabilityKey) -> Vec<ComputeCandidate> {
        let Some(matches) = self.capabilities.get(&key.op) else {
            return vec![];
        };
        let explicit = matches
            .iter()
            .filter(|candidate| {
                candidate.execution == key.execution
                    && candidate.arity == key.inputs.len()
                    && candidate.inputs.as_ref() == Some(&key.inputs)
            })
            .collect::<Vec<_>>();
        let candidates = if explicit.is_empty() {
            matches
                .iter()
                .filter(|candidate| {
                    candidate.execution == key.execution
                        && candidate.arity == key.inputs.len()
                        && candidate.inputs.is_none()
                })
                .collect::<Vec<_>>()
        } else {
            explicit
        };
        candidates
            .into_iter()
            .map(|candidate| ComputeCandidate {
                engine: candidate.engine,
                rate: candidate.rate,
                flops_per_op: candidate.flops_per_op,
                matrix_service_tile: candidate.matrix_service_tile,
                matrix_accumulator_capacity_bytes: self.matrix_accumulator_capacities
                    [candidate.engine.0],
            })
            .collect()
    }
}

/// PE compute hardware. No operation routing table is needed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeConfig {
    /// Named independent engines.
    #[serde(deserialize_with = "unique_map")]
    pub engines: BTreeMap<String, EngineConfig>,
}

impl ComputeConfig {
    #[must_use]
    pub fn classes_used(&self) -> BTreeSet<&str> {
        self.engines
            .values()
            .flat_map(|engine| engine.capabilities.iter().map(|cap| cap.class.as_str()))
            .collect()
    }
}

/// One engine: all its typed capabilities share a single batch slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    /// Caller engine-local logical accumulator capacity; independent of SRAM
    /// storage and service padding. Omitted preserves unconstrained admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_accumulator_capacity_bytes: Option<u64>,
    /// Compact capability declarations, preserved by YAML serialization.
    pub capabilities: Vec<CapabilityConfig>,
}

/// A compact declaration of typed operations sharing one throughput rate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityConfig {
    /// Platform-wide operation class.
    pub class: String,
    /// Exact ordered input datatype combinations. Omit to cover
    /// combinations not explicitly declared for this class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_dtypes: Option<Vec<Vec<DataType>>>,
    /// Full-engine logical operations per tick.
    pub ops_per_tick: f64,
    /// Optional exact execution semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionMode>,
    /// Caller matrix macro shape used only for charged service work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_service_tile: Option<MatrixServiceTile>,
}

impl CapabilityConfig {
    pub fn validate(&self, class: &OperationClass) -> Result<(), SimError> {
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
        if let Some(tile) = self.matrix_service_tile {
            tile.volume()?;
            if class.arity != 2
                || class.operations != ["matrix_mac"]
                || self.input_dtypes.is_none()
                || self.execution.as_ref().is_none_or(|mode| {
                    mode.instruction.trim().is_empty() || mode.sparsity != Sparsity::Dense
                })
            {
                return sim_error!(
                    "matrix service tile requires exact dense matrix_mac class/operands/execution"
                );
            }
        }
        Ok(())
    }
}

/// Caller service macro dimensions; not physical vendor padding or traffic.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixServiceTile {
    pub m: usize,
    pub n: usize,
    pub k: usize,
}

impl MatrixServiceTile {
    fn volume(self) -> Result<usize, SimError> {
        MatrixWorkShape {
            m: self.m,
            n: self.n,
            k: self.k,
        }
        .logical_macs()
    }

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
            m: rounded(shape.m, self.m)?,
            n: rounded(shape.n, self.n)?,
            k: rounded(shape.k, self.k)?,
        }
        .logical_macs()
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
    matrix_service_tile: Option<MatrixServiceTile>,
    execution: Option<ExecutionMode>,
    engine: EngineId,
    rate: f64,
    inputs: Option<Vec<DataType>>,
    arity: usize,
    flops_per_op: usize,
}

/// Private selected service option; logical flop cost is independent of policy.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ComputeCandidate {
    pub matrix_accumulator_capacity_bytes: Option<u64>,
    pub engine: EngineId,
    pub rate: f64,
    pub flops_per_op: usize,
    pub matrix_service_tile: Option<MatrixServiceTile>,
}

/// Instruction semantics independent of tensor storage format.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionMode {
    /// Platform-defined instruction family, for example `tensor` or `simt`.
    pub instruction: String,
    /// Arithmetic precision retained between reduction chunks.
    pub accumulator: DataType,
    /// Required operand sparsity; sparse attention does not imply sparse GEMM.
    #[serde(default)]
    pub sparsity: Sparsity,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sparsity {
    #[default]
    Dense,
    Structured,
}

/// Stable engine index in a validated model.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct EngineId(pub usize);

fn one_flop() -> usize {
    1
}

pub(crate) fn is_lowercase_operation(name: &str) -> bool {
    !name.trim().is_empty() && name == name.to_lowercase()
}

// Reject duplicate map keys instead of allowing serde maps to overwrite them.
fn unique_map<'de, D, V>(deserializer: D) -> Result<BTreeMap<String, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct Visitor<V>(std::marker::PhantomData<V>);
    impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<V> {
        type Value = BTreeMap<String, V>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a map with unique names")
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut values = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, V>()? {
                if values.insert(key.clone(), value).is_some() {
                    return Err(serde::de::Error::custom(format!("duplicate key '{key}'")));
                }
            }
            Ok(values)
        }
    }
    deserializer.deserialize_map(Visitor(std::marker::PhantomData))
}
