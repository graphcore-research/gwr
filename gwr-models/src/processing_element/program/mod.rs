// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Structured tensor programs executed by a processing element.
//!
//! A boundary tensor may have a distinct mapping in each assignment. Reads
//! inside reductions are streamed in bounded chunks; subsequent consumers
//! reread their own mapped views after their producer dependencies complete.
//! Scalar expression trees lower to ordered compute batches with explicit
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
//! tensor evaluation.
//!
//! Assignment dependencies and `tiling.sizes` determine execution phases.
//! Completed row reductions run once before their results are broadcast across
//! output chunks. Ready assignments with matching free axes share a
//! dependency-driven tile group, preserving local buffer reuse, shared initial
//! loads and independent execution unit overlap.
//!
//! A local tensor consumed across phases is retained in the least enclosing
//! scope of its producer and consumers. If a later reduction needs a complete
//! local row, its producer finishes that row before the reduction starts.
//! Scratch storage is reused between groups; retained values remain live until
//! their last use. Capacity checks reserve a safe peak arena before execution.
//! Insufficient SRAM is an error: execution does not recompute earlier phases
//! or spill local values to external memory.

pub(super) mod plan;
pub(crate) mod schedule;
pub(super) mod storage;
pub use storage::SRAM_SLOT_ALIGNMENT_BYTES;
pub mod syntax;
pub mod tile;
pub mod validate;

#[cfg(test)]
mod storage_tests;
#[cfg(test)]
mod tile_tests;
#[cfg(test)]
mod validation_tests;
