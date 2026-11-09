// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Structured tensor programs executed by a processing element.
//!
//! A boundary tensor may have a distinct mapping in each assignment. Reads
//! inside reductions are streamed in bounded chunks; subsequent consumers
//! reread their own mapped views after their producer dependencies complete.
//! Scalar expression trees lower to ordered engine batches with explicit
//! intermediate chunk storage.
//!
//! Scalar reads retain their declared tensor dtype. Every scalar call result,
//! including nested calls, uses the enclosing assignment's destination dtype;
//! FP32 leaves do not promote intermediate results. Wrapping an expression in
//! `convert` does not create a separate dtype scope for its child. To preserve
//! FP32 opmath before a BF16 store, assign the expression to a FP32 temporary,
//! then explicitly `convert` that temporary in the BF16 output assignment.
//! Named work signatures match operand dtypes; implicit call-result transitions
//! do not add conversion work. This is a modeled cost contract, not numerical
//! tensor evaluation. The opt-in [`scalar_dtype`] guard checks caller-selected
//! homogeneous scalar assignments without changing ordinary validation.
//!
//! When an output axis also occurs in an earlier reduction, output tiling uses
//! **recompute and reread**: each output tile performs complete streamed row
//! reductions, retains local scalar results, then rereads its output chunk.
//! Arithmetic and boundary traffic therefore grow with the number of output
//! chunks. This bounded strategy is deliberate, rather than free reuse of a
//! reduction result across independent tiles. Full-row local materialization
//! followed by partial-row production is rejected; inline scalar expressions
//! allow normalization without that materialization.

pub mod block_plan;
pub(crate) mod indexed_metadata;
pub mod pipeline;
pub mod plan;
pub mod scalar_dtype;
mod scaled;
pub mod storage;
pub mod syntax;
pub mod tile;
pub mod validate;
