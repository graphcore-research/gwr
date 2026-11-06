// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Closed executable grammar. Serialized expressions never reach the runner.
use std::collections::{BTreeMap, BTreeSet};

use gwr_engine::types::SimError;

use super::syntax::{self, BoundsMode, IndexSpec, ReductionOp, TemporarySpec};
use crate::processing_element::compute::is_lowercase_operation;
use crate::processing_element::tensors::dtype::DataType;

#[derive(Clone, Debug)]
pub(in crate::processing_element) struct Program {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub temporaries: BTreeMap<String, TemporarySpec>,
    pub assignments: Vec<Assignment>,
}
#[derive(Clone, Debug)]
pub(in crate::processing_element) struct Assignment {
    results: Results,
    pub axes: Vec<String>,
    pub expr: Expr,
}
#[derive(Clone, Debug)]
enum Results {
    Value(String),
    Index(String),
    Both(String, String),
}
impl Assignment {
    pub fn output(&self) -> Option<&String> {
        match &self.results {
            Results::Value(name) | Results::Both(name, _) => Some(name),
            Results::Index(_) => None,
        }
    }
    pub fn indices_output(&self) -> Option<&String> {
        match &self.results {
            Results::Index(name) | Results::Both(_, name) => Some(name),
            Results::Value(_) => None,
        }
    }
    pub fn primary_result(&self) -> &str {
        self.output().or_else(|| self.indices_output()).unwrap()
    }
}
#[derive(Clone, Debug)]
pub(in crate::processing_element) enum Expr {
    Scalar(ScalarExpr),
    Reduce {
        op: ReductionOp,
        over: Vec<String>,
        value: ScalarExpr,
    },
    MatrixMac {
        accumulator: DataType,
        over: [String; 1],
        args: [Read; 2],
    },
}
#[derive(Clone, Debug)]
pub(in crate::processing_element) enum ScalarExpr {
    Read(Read),
    Call { op: String, args: Vec<ScalarExpr> },
}
#[derive(Clone, Debug)]
pub(in crate::processing_element) struct Read {
    pub tensor: String,
    pub indices: Vec<IndexSpec>,
    pub bounds: BoundsMode,
}
impl Expr {
    pub fn reduction_axes(&self) -> &[String] {
        match self {
            Self::Reduce { over, .. } => over,
            Self::MatrixMac { over, .. } => over,
            Self::Scalar(_) => &[],
        }
    }
    #[cfg(test)]
    pub fn scalar(&self) -> Option<&ScalarExpr> {
        match self {
            Self::Scalar(value) | Self::Reduce { value, .. } => Some(value),
            Self::MatrixMac { .. } => None,
        }
    }
    pub fn iter_reads(&self) -> impl Iterator<Item = &Read> {
        let (scalar, matrix): (Option<&ScalarExpr>, &[Read]) = match self {
            Self::Scalar(value) | Self::Reduce { value, .. } => (Some(value), &[]),
            Self::MatrixMac { args, .. } => (None, args),
        };
        scalar
            .into_iter()
            .flat_map(ScalarExpr::iter_reads)
            .chain(matrix)
    }
    pub fn iter_read_tensor_names(&self) -> impl Iterator<Item = &str> {
        self.iter_reads().map(|read| read.tensor.as_str())
    }
    pub fn collect_read_tensor_names(&self) -> BTreeSet<String> {
        self.iter_read_tensor_names().map(str::to_owned).collect()
    }
}
impl ScalarExpr {
    pub fn iter_reads(&self) -> impl Iterator<Item = &Read> {
        let mut pending = vec![self];
        std::iter::from_fn(move || {
            while let Some(expr) = pending.pop() {
                match expr {
                    Self::Read(read) => return Some(read),
                    Self::Call { args, .. } => pending.extend(args.iter().rev()),
                }
            }
            None
        })
    }
}
impl Program {
    pub fn bind(spec: &syntax::ProgramSpec) -> Result<Self, SimError> {
        let assignments = spec
            .assignments
            .iter()
            .map(|raw| {
                let expr = match &raw.expr {
                    syntax::ExprSpec::Reduce { op, over, value } => {
                        if matches!(op, ReductionOp::Min | ReductionOp::Max) {
                            super::validate::find_common_excluded_read(value)?;
                        }
                        Expr::Reduce {
                            op: *op,
                            over: over.clone(),
                            value: scalar(value)?,
                        }
                    }
                    syntax::ExprSpec::MatrixMac {
                        accumulator,
                        over,
                        args,
                    } => {
                        super::validate::validate_matrix_expression(args, over)?;
                        Expr::MatrixMac {
                            accumulator: *accumulator,
                            over: [over[0].clone()],
                            args: [read(&args[0])?, read(&args[1])?],
                        }
                    }
                    expr => Expr::Scalar(scalar(expr)?),
                };
                let results = match (&raw.output, &raw.indices_output) {
                    (Some(value), None) => Results::Value(value.clone()),
                    (None, Some(index))
                        if matches!(
                            expr,
                            Expr::Reduce {
                                op: ReductionOp::Min | ReductionOp::Max,
                                ..
                            }
                        ) =>
                    {
                        Results::Index(index.clone())
                    }
                    (Some(value), Some(index))
                        if matches!(
                            expr,
                            Expr::Reduce {
                                op: ReductionOp::Min | ReductionOp::Max,
                                ..
                            }
                        ) =>
                    {
                        Results::Both(value.clone(), index.clone())
                    }
                    _ => return Err(SimError("invalid assignment result combination".into())),
                };
                Ok(Assignment {
                    results,
                    axes: raw.axes.clone(),
                    expr,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            inputs: spec.inputs.clone(),
            outputs: spec.outputs.clone(),
            temporaries: spec.temporaries.clone(),
            assignments,
        })
    }
}
fn scalar(expr: &syntax::ExprSpec) -> Result<ScalarExpr, SimError> {
    match expr {
        syntax::ExprSpec::Read {..} => Ok(ScalarExpr::Read(read(expr)?)),
        syntax::ExprSpec::Call {op, args} => {
            if !is_lowercase_operation(op) {return Err(SimError(format!("operation '{op}' must be lowercase")));}
            if matches!(op.as_str(), "read" | "reduce" | "matrix_mac") {return Err(SimError(format!("operation '{op}' uses a reserved expression name")));}
            if args.is_empty() {return Err(SimError("unsupported empty scalar call".into()));}
            Ok(ScalarExpr::Call {op: op.clone(), args: args.iter().map(scalar).collect::<Result<_, _>>()?})
        }
        _ => Err(SimError("unsupported nested reduction or matrix arithmetic; use an explicit temporary assignment".into())),
    }
}
fn read(expr: &syntax::ExprSpec) -> Result<Read, SimError> {
    match expr {
        syntax::ExprSpec::Read {
            tensor,
            indices,
            bounds,
        } => Ok(Read {
            tensor: tensor.clone(),
            indices: indices.clone(),
            bounds: *bounds,
        }),
        _ => Err(SimError("matrix operand must be a direct read".into())),
    }
}
