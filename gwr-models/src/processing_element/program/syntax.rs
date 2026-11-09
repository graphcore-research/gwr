// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Serialized tensor-program syntax. Semantic validation lives in the region
//! planner.

use std::collections::BTreeMap;

use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::processing_element::tensors::dtype::DataType;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramSpec {
    pub inputs: Vec<String>,
    /// Full-view metadata loaded before arithmetic in every output tile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metadata_inputs: Vec<String>,
    /// Optional scalar Uint8 scale-plane range union; full backing/SRAM
    /// unchanged.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_indexed_metadata_inputs"
    )]
    pub indexed_metadata_inputs: Vec<String>,
    /// Exact caller logical local-byte recipe; None preserves ordinary
    /// execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_round_policy: Option<LocalRoundPolicy>,
    pub outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub temporaries: BTreeMap<String, TemporarySpec>,
    pub assignments: Vec<AssignmentSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extents: BTreeMap<String, usize>,
}

/// Read the opt-in metadata policy as an actual sequence of string values.
///
/// YAML's ordinary String deserializer accepts scalar booleans/numbers as
/// text. This new policy deliberately requires typed strings; missing fields
/// still use the empty default. Shared by both serialized program carriers.
pub fn deserialize_indexed_metadata_inputs<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let yaml_serde::Value::Sequence(values) = yaml_serde::Value::deserialize(deserializer)? else {
        return Err(D::Error::custom(
            "indexed_metadata_inputs requires a sequence of strings",
        ));
    };
    values
        .into_iter()
        .map(|value| match value {
            yaml_serde::Value::String(name) => Ok(name),
            _ => Err(D::Error::custom(
                "indexed_metadata_inputs requires string members",
            )),
        })
        .collect()
}

/// This first increment admits the exact source E8M0 reference only for b=1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalRoundPolicy {
    V41RoundScaleScalar,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TemporarySpec {
    pub dtype: DataType,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indices_output: Option<String>,
    pub axes: Vec<String>,
    pub expr: ExprSpec,
}

impl AssignmentSpec {
    #[must_use]
    pub fn primary_result(&self) -> String {
        self.output
            .as_deref()
            .or(self.indices_output.as_deref())
            .expect("validated assignment has a result")
            .to_owned()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprSpec {
    Read {
        tensor: String,
        indices: Vec<IndexSpec>,
        bounds: BoundsMode,
    },
    Call {
        op: String,
        args: Vec<ExprSpec>,
        attrs: BTreeMap<String, yaml_serde::Value>,
    },
    MatrixMac {
        execution: Option<super::super::compute::ExecutionMode>,
        args: Vec<ExprSpec>,
        over: Vec<String>,
        /// Optional packed operand formats and mandatory scale-load metadata.
        quantization: Option<[ScaledOperandSpec; 2]>,
    },
    Reduce {
        op: ReductionOp,
        over: Vec<String>,
        value: Box<ExprSpec>,
    },
}

impl<'de> Deserialize<'de> for ExprSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let yaml_serde::Value::Mapping(mapping) = yaml_serde::Value::deserialize(deserializer)?
        else {
            return Err(D::Error::custom("expression must be a one-key mapping"));
        };
        if mapping.len() != 1 {
            return Err(D::Error::custom("expression must be a one-key mapping"));
        }
        let (key, payload) = mapping.into_iter().next().unwrap();
        let yaml_serde::Value::String(op) = key else {
            return Err(D::Error::custom("expression key must be an operation name"));
        };
        match op.as_str() {
            "read" => {
                let read: ReadRepr = yaml_serde::from_value(payload).map_err(D::Error::custom)?;
                Ok(Self::Read {
                    tensor: read.tensor,
                    indices: read.indices,
                    bounds: read.bounds,
                })
            }
            "reduce" => {
                let reduce: ReduceRepr =
                    yaml_serde::from_value(payload).map_err(D::Error::custom)?;
                Ok(Self::Reduce {
                    op: reduce.op,
                    over: reduce.over,
                    value: reduce.value,
                })
            }
            "matrix_mac" => {
                let mac: MatrixMacRepr =
                    yaml_serde::from_value(payload).map_err(D::Error::custom)?;
                Ok(Self::MatrixMac {
                    execution: mac.execution,
                    args: mac.args,
                    over: mac.over,
                    quantization: mac.quantization,
                })
            }
            _ => {
                let call: CallPayload =
                    yaml_serde::from_value(payload).map_err(D::Error::custom)?;
                let CallRepr { args, attrs } = match call {
                    CallPayload::Args(args) => CallRepr {
                        args,
                        attrs: BTreeMap::new(),
                    },
                    CallPayload::Full(call) => call,
                };
                Ok(Self::Call { op, args, attrs })
            }
        }
    }
}

impl Serialize for ExprSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let (name, payload) = match self {
            Self::Read {
                tensor,
                indices,
                bounds,
            } => (
                "read".to_owned(),
                yaml_serde::to_value(ReadRepr {
                    tensor: tensor.clone(),
                    indices: indices.clone(),
                    bounds: *bounds,
                })
                .map_err(S::Error::custom)?,
            ),
            Self::Reduce { op, over, value } => (
                "reduce".to_owned(),
                yaml_serde::to_value(ReduceRepr {
                    op: *op,
                    over: over.clone(),
                    value: value.clone(),
                })
                .map_err(S::Error::custom)?,
            ),
            Self::MatrixMac {
                args,
                over,
                execution,
                quantization,
            } => (
                "matrix_mac".to_owned(),
                yaml_serde::to_value(MatrixMacRepr {
                    execution: execution.clone(),
                    args: args.clone(),
                    over: over.clone(),
                    quantization: quantization.clone(),
                })
                .map_err(S::Error::custom)?,
            ),
            Self::Call { op, args, attrs } => {
                let payload = if attrs.is_empty() {
                    CallPayload::Args(args.clone())
                } else {
                    CallPayload::Full(CallRepr {
                        args: args.clone(),
                        attrs: attrs.clone(),
                    })
                };
                (
                    op.clone(),
                    yaml_serde::to_value(payload).map_err(S::Error::custom)?,
                )
            }
        };
        let mut mapping = yaml_serde::Mapping::new();
        mapping.insert(yaml_serde::Value::String(name), payload);
        mapping.serialize(serializer)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CallRepr {
    args: Vec<ExprSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    attrs: BTreeMap<String, yaml_serde::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MatrixMacRepr {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution: Option<super::super::compute::ExecutionMode>,
    #[serde(default)]
    over: Vec<String>,
    args: Vec<ExprSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quantization: Option<[ScaledOperandSpec; 2]>,
}

/// Packed E2M1 value encoding with format-specific one-byte block scales.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PackedMatrixFormat {
    /// Sixteen values share an E4M3 scale plus one FP32 scale per tensor.
    Nvfp4,
    /// Thirty-two values share a power-of-two E8M0 scale.
    Mxfp4,
}

impl PackedMatrixFormat {
    /// Required values per block, independent of physical tail padding.
    #[must_use]
    pub fn block_size(self) -> usize {
        match self {
            Self::Nvfp4 => 16,
            Self::Mxfp4 => 32,
        }
    }
    /// Exact matrix instruction tag for this operand pair.
    #[must_use]
    pub fn instruction(self) -> &'static str {
        match self {
            Self::Nvfp4 => "nvfp4",
            Self::Mxfp4 => "mxfp4",
        }
    }
}

/// Physical scale-plane allocation; metadata loads read the full actual view
/// once per output tile, including padding. Scale swizzle order is not
/// simulated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScaleLayout {
    /// One byte per logical block, with checked partial-tail ceil division.
    #[default]
    Logical,
    /// Two-dimensional scale plane padded to 128 rows and four block columns.
    Padded128x4,
}

/// One packed operand's scale contract. Scale tensor names must be boundary
/// inputs in `metadata_inputs`, ensuring real LSU loads before arithmetic.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScaledOperandSpec {
    /// Numeric format, distinct from the generic Fp4 storage dtype.
    pub format: PackedMatrixFormat,
    /// Must equal the format's required sixteen or thirty-two value block.
    pub block_size: usize,
    /// One-byte block scale input: E4M3 for NVFP4, E8M0 for MXFP4.
    pub block_scales: String,
    /// Mandatory FP32 scalar input for NVFP4; forbidden for MXFP4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_scale: Option<String>,
    /// Physical scale allocation shape and conservative read contract.
    #[serde(default)]
    pub layout: ScaleLayout,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum CallPayload {
    Args(Vec<ExprSpec>),
    Full(CallRepr),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReduceRepr {
    op: ReductionOp,
    over: Vec<String>,
    value: Box<ExprSpec>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReadRepr {
    tensor: String,
    indices: Vec<IndexSpec>,
    #[serde(default)]
    bounds: BoundsMode,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum IndexSpec {
    Axis(String),
    Zero(usize),
    Affine { affine: AffineSpec },
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AffineSpec {
    pub terms: BTreeMap<String, usize>,
    pub offset: i64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundsMode {
    #[default]
    RequireInRange,
    Exclude,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReductionOp {
    Sum,
    Max,
    Min,
}

impl ReductionOp {
    #[must_use]
    pub fn is_extremum(self) -> bool {
        matches!(self, Self::Max | Self::Min)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TileSpec {
    #[serde(default)]
    pub sizes: BTreeMap<String, usize>,
}
