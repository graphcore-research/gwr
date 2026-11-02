// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Serialized tensor-program syntax. Semantic validation lives in the region
//! planner.

use std::collections::{BTreeMap, BTreeSet};
use std::iter::from_fn;

use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use yaml_serde::{Mapping, Value, from_value, to_value};

use crate::processing_element::tensors::dtype::DataType;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramSpec {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub temporaries: BTreeMap<String, TemporarySpec>,
    pub assignments: Vec<AssignmentSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extents: BTreeMap<String, usize>,
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
    pub(in crate::processing_element) fn primary_result(&self) -> String {
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
    },
    MatrixMac {
        /// Arithmetic precision retained across contraction chunks.
        accumulator: DataType,
        args: Vec<ExprSpec>,
        over: Vec<String>,
    },
    Reduce {
        op: ReductionOp,
        over: Vec<String>,
        value: Box<ExprSpec>,
    },
}

impl ExprSpec {
    pub(crate) fn collect_read_tensor_names(&self) -> BTreeSet<String> {
        self.iter_read_tensor_names().map(str::to_owned).collect()
    }

    /// Borrow tensor names in declaration order, including repeated reads.
    pub(crate) fn iter_read_tensor_names(&self) -> impl Iterator<Item = &str> {
        self.iter_reads().map(|expr| {
            let Self::Read { tensor, .. } = expr else {
                unreachable!()
            };
            tensor.as_str()
        })
    }

    /// Read leaves in declaration order, including repeated reads.
    pub(crate) fn iter_reads(&self) -> impl Iterator<Item = &Self> {
        self.walk().filter(|expr| matches!(expr, Self::Read { .. }))
    }

    /// Visit this expression, then its children in declaration order.
    pub(crate) fn walk(&self) -> impl Iterator<Item = &Self> {
        let mut pending = vec![self];
        from_fn(move || {
            let expr = pending.pop()?;
            match expr {
                Self::Call { args, .. } | Self::MatrixMac { args, .. } => {
                    pending.extend(args.iter().rev());
                }
                Self::Reduce { value, .. } => pending.push(value),
                Self::Read { .. } => {}
            }
            Some(expr)
        })
    }
}

impl<'de> Deserialize<'de> for ExprSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let Value::Mapping(mapping) = Value::deserialize(deserializer)? else {
            return Err(D::Error::custom("expression must be a one-key mapping"));
        };
        if mapping.len() != 1 {
            return Err(D::Error::custom("expression must be a one-key mapping"));
        }
        let (key, payload) = mapping.into_iter().next().unwrap();
        let Value::String(op) = key else {
            return Err(D::Error::custom("expression key must be an operation name"));
        };
        match op.as_str() {
            "read" => {
                let read: ReadRepr = from_value(payload).map_err(D::Error::custom)?;
                Ok(Self::Read {
                    tensor: read.tensor,
                    indices: read.indices,
                    bounds: read.bounds,
                })
            }
            "reduce" => {
                let reduce: ReduceRepr = from_value(payload).map_err(D::Error::custom)?;
                Ok(Self::Reduce {
                    op: reduce.op,
                    over: reduce.over,
                    value: reduce.value,
                })
            }
            "matrix_mac" => {
                let mac: MatrixMacRepr = from_value(payload).map_err(D::Error::custom)?;
                Ok(Self::MatrixMac {
                    accumulator: mac.accumulator,
                    args: mac.args,
                    over: mac.over,
                })
            }
            _ => {
                let args = from_value(payload).map_err(D::Error::custom)?;
                Ok(Self::Call { op, args })
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
                to_value(ReadRepr {
                    tensor: tensor.clone(),
                    indices: indices.clone(),
                    bounds: *bounds,
                })
                .map_err(S::Error::custom)?,
            ),
            Self::Reduce { op, over, value } => (
                "reduce".to_owned(),
                to_value(ReduceRepr {
                    op: *op,
                    over: over.clone(),
                    value: value.clone(),
                })
                .map_err(S::Error::custom)?,
            ),
            Self::MatrixMac {
                args,
                over,
                accumulator,
            } => (
                "matrix_mac".to_owned(),
                to_value(MatrixMacRepr {
                    accumulator: *accumulator,
                    args: args.clone(),
                    over: over.clone(),
                })
                .map_err(S::Error::custom)?,
            ),
            Self::Call { op, args } => (op.clone(), to_value(args).map_err(S::Error::custom)?),
        };
        let mut mapping = Mapping::new();
        mapping.insert(Value::String(name), payload);
        mapping.serialize(serializer)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MatrixMacRepr {
    accumulator: DataType,
    #[serde(default)]
    over: Vec<String>,
    args: Vec<ExprSpec>,
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

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TileSpec {
    #[serde(default)]
    pub sizes: BTreeMap<String, usize>,
}
