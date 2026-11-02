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

/// Named boundary tensors and assignments forming one tensor program.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramSpec {
    /// Input tensor names, in boundary binding order.
    pub inputs: Vec<String>,
    /// Output tensor names, in boundary binding order.
    pub outputs: Vec<String>,
    /// Local values whose shapes are inferred from their producing assignments.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub temporaries: BTreeMap<String, TemporarySpec>,
    /// Assignments ordered by their validated tensor dependencies for
    /// execution.
    pub assignments: Vec<AssignmentSpec>,
    /// Explicit axis extents supplementing constraints from boundary inputs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extents: BTreeMap<String, usize>,
}

/// Storage datatype for an assignment-produced local tensor.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TemporarySpec {
    /// Datatype used by the local value and its scalar expression results.
    pub dtype: DataType,
}

/// One value-producing expression and its free output axes.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentSpec {
    /// Value output name; may be omitted for an index-only extremum reduction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Optional index output of a minimum or maximum reduction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indices_output: Option<String>,
    /// Free axes determining output shape in dimension order.
    pub axes: Vec<String>,
    /// Expression whose work and reads are modeled for each output tile.
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

/// Serialized expression tree lowered to typed compute work and tensor reads.
#[derive(Clone, Debug, PartialEq)]
pub enum ExprSpec {
    /// Read a boundary tensor or a previously produced local value.
    Read {
        /// Declared name of the tensor to read.
        tensor: String,
        /// Logical index for each tensor dimension.
        indices: Vec<IndexSpec>,
        /// Treatment of coordinates outside the tensor shape.
        bounds: BoundsMode,
    },
    /// Apply a named scalar operation to its operands.
    Call {
        /// Lowercase operation name matched against compute capabilities.
        op: String,
        /// Operands in the order used for datatype signature matching.
        args: Vec<ExprSpec>,
    },
    /// Contract two directly indexed tensor operands along one reduction axis.
    MatrixMac {
        /// Arithmetic precision retained across contraction chunks.
        accumulator: DataType,
        /// Two direct reads with in-range, non-affine indices.
        args: Vec<ExprSpec>,
        /// Single axis shared by both operands and reduced by the contraction.
        over: Vec<String>,
    },
    /// Reduce a scalar expression over the declared axes.
    Reduce {
        /// Accumulator operation for the reduction.
        op: ReductionOp,
        /// Axes consumed by the reduction rather than retained in the output.
        over: Vec<String>,
        /// Scalar expression evaluated at each selected reduction coordinate.
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

/// One logical tensor-dimension index.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum IndexSpec {
    /// Coordinate of a named axis, broadcast when the tensor dimension is one.
    Axis(String),
    /// Literal zero selecting a singleton dimension; other values are rejected.
    Zero(usize),
    /// Weighted axis coordinates plus a signed offset.
    Affine {
        /// Coefficients and offset for the coordinate expression.
        affine: AffineSpec,
    },
}

/// Affine coordinate expression with positive integer axis coefficients.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AffineSpec {
    /// Named axes and their nonzero coefficients.
    pub terms: BTreeMap<String, usize>,
    /// Signed offset added to the weighted coordinates.
    pub offset: i64,
}

/// Handling of tensor coordinates outside the declared shape.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundsMode {
    /// Require every selected coordinate to be in range during validation.
    #[default]
    RequireInRange,
    /// Exclude out-of-range coordinates from a reduction's body and work count.
    Exclude,
}

/// Operation combining selected scalar values into a reduction result.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReductionOp {
    /// Sum the selected values.
    Sum,
    /// Select the maximum value and, when requested, its index.
    Max,
    /// Select the minimum value and, when requested, its index.
    Min,
}

/// Requested axis chunk sizes for output tiles and reduction invocations.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TileSpec {
    /// Positive sizes by axis name; omitted axes use their full resolved
    /// extent.
    #[serde(default)]
    pub sizes: BTreeMap<String, usize>,
}
