// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

pub use super::runner::streamed_matrix::{StreamedMatrixTaskConfig, StreamedOutputSlot};
use crate::processing_element::program::syntax::{ProgramSpec, TileSpec};
use crate::processing_element::tensors::TensorView;

#[derive(Debug, Clone)]
pub enum Task {
    StreamedMatrix {
        config: std::rc::Rc<super::runner::streamed_matrix::StreamedOutputSlot>,
    },
    AtomicEffect {
        config: Box<AtomicTaskConfig>,
    },
    AtomicControlJoin {
        config: Box<AtomicJoinConfig>,
    },
    ProgramTask {
        config: Box<ProgramTaskConfig>,
    },
    TransferTask {
        config: Box<crate::transfer::TransferTaskConfig>,
    },
    SyncTask {
        region: SyncRegion,
    },
}

#[derive(Debug, Clone)]
pub struct ProgramTaskConfig {
    pub id: String,
    pub program: ProgramSpec,
    pub tiling: Option<TileSpec>,
    /// Explicit bounded block schedule; absence preserves legacy execution.
    pub pipeline: Option<super::program::pipeline::BlockPipelineSpec>,
    pub inputs: Vec<TensorView>,
    pub outputs: Vec<TensorView>,
}

#[derive(Debug, Clone, Copy)]
pub enum SyncRegion {
    Local,
    Global,
}

/// Opaque admitted static dispatch. Clones do not mint owners or source IDs.
#[derive(Clone)]
pub struct AtomicTaskConfig {
    pub id: String,
    target: std::rc::Rc<crate::memory::atomic::AtomicTargetSender>,
    device: u64,
    address: u64,
    effect: crate::memory::atomic::Effect,
    issuer: Option<std::rc::Weak<super::ProcessingElement>>,
    controller: Option<std::rc::Rc<crate::memory::atomic::SplitKController>>,
    source_operand: Option<(
        std::rc::Rc<super::runner::streamed_matrix::StreamedOutputSlot>,
        usize,
    )>,
}
impl std::fmt::Debug for AtomicTaskConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AtomicTaskConfig")
            .field("id", &self.id)
            .field("device", &self.device)
            .field("address", &self.address)
            .field("physical_issuer_bound", &self.issuer.is_some())
            .finish()
    }
}
impl AtomicTaskConfig {
    /// Bind only an actual PE identity, never a caller-provided proof callback.
    /// Direct request/reply identity is independently verified by the private
    /// models-owned endpoints captured from successful physical connections.
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use gwr_models::processing_element::task::AtomicTaskConfig;
    /// fn forge(config:AtomicTaskConfig) {config.with_physical_issuer(Rc::new(|_|Ok(())));}
    /// ```
    pub fn with_physical_issuer(mut self, pe: &std::rc::Rc<super::ProcessingElement>) -> Self {
        self.issuer = Some(std::rc::Rc::downgrade(pe));
        self
    }
    pub fn validate_for_pe(&self, pe: &super::ProcessingElement) -> gwr_engine::types::SimResult {
        self.validate()?;
        if let Some((slot, _)) = &self.source_operand {
            slot.validate_for_pe(pe)?;
        }
        pe.validate_atomic_word_mapping(self.device, self.address)?;
        let expected = self
            .issuer
            .as_ref()
            .and_then(std::rc::Weak::upgrade)
            .ok_or_else(|| {
                gwr_engine::types::SimError(
                    "atomic task lacks actual platform issuer binding".into(),
                )
            })?;
        if !std::ptr::eq(expected.as_ref(), pe) {
            return Err(gwr_engine::types::SimError(
                "atomic task activated on a foreign PE".into(),
            ));
        }
        pe.validate_atomic_direct_target(&self.target)
    }
    pub fn new(
        id: String,
        target: std::rc::Rc<crate::memory::atomic::AtomicTargetSender>,
        device: u64,
        address: u64,
        effect: crate::memory::atomic::Effect,
    ) -> Result<Self, gwr_engine::types::SimError> {
        let config = Self {
            id,
            target,
            device,
            address,
            effect,
            issuer: None,
            controller: None,
            source_operand: None,
        };
        config.validate()?;
        Ok(config)
    }
    pub fn new_controlled(
        id: String,
        target: std::rc::Rc<crate::memory::atomic::AtomicTargetSender>,
        device: u64,
        address: u64,
        effect: crate::memory::atomic::Effect,
        controller: std::rc::Rc<crate::memory::atomic::SplitKController>,
    ) -> Result<Self, gwr_engine::types::SimError> {
        if !controller.targets(&target) || controller.group().device != device {
            return Err(gwr_engine::types::SimError(
                "controller target differs from actual task target".into(),
            ));
        }
        let config = Self {
            id,
            target,
            device,
            address,
            effect,
            issuer: None,
            controller: Some(controller),
            source_operand: None,
        };
        config.validate()?;
        Ok(config)
    }
    pub fn with_streamed_operand(
        mut self,
        slot: std::rc::Rc<super::runner::streamed_matrix::StreamedOutputSlot>,
        offset: usize,
    ) -> Result<Self, gwr_engine::types::SimError> {
        slot.validate_target(&self.target)?;
        slot.validate_word(self.effect, offset)?;
        self.source_operand = Some((slot, offset));
        Ok(self)
    }
    pub fn validate(&self) -> gwr_engine::types::SimResult {
        self.target.path().validate()?;
        if let Some((slot, offset)) = &self.source_operand {
            slot.validate_target(&self.target)?;
            slot.validate_word(self.effect, *offset)?;
        }
        if self.id.is_empty() {
            return Err(gwr_engine::types::SimError(
                "atomic task ID cannot be empty".into(),
            ));
        }
        if let Some(controller) = &self.controller {
            controller.validate_role(self.address, self.effect)
        } else {
            self.target
                .validate_effect(self.device, self.address, self.effect)
        }
    }
    pub async fn execute(
        &self,
    ) -> Result<crate::memory::atomic::Completion, gwr_engine::types::SimError> {
        self.validate()?;
        let operand = if let Some((slot, offset)) = &self.source_operand {
            Some(slot.take(self.effect, *offset)?)
        } else {
            None
        };
        if let Some(controller) = &self.controller {
            controller
                .execute_with_operand(self.address, self.effect, operand)
                .await
        } else {
            self.target
                .receive_with_operand(self.device, self.address, self.effect, operand)
                .await?
                .acknowledge()
                .await
        }
    }
}

#[derive(Clone, Debug)]
pub struct AtomicJoinConfig {
    pub(crate) binding: AtomicTaskConfig,
    controller: std::rc::Rc<crate::memory::atomic::SplitKController>,
}
impl AtomicJoinConfig {
    pub fn new(
        binding: AtomicTaskConfig,
        controller: std::rc::Rc<crate::memory::atomic::SplitKController>,
    ) -> Result<Self, gwr_engine::types::SimError> {
        if !controller.targets(&binding.target)
            || !binding
                .controller
                .as_ref()
                .is_some_and(|actual| std::rc::Rc::ptr_eq(actual, &controller))
        {
            return Err(gwr_engine::types::SimError(
                "join target/controller mismatch".into(),
            ));
        }
        Ok(Self {
            binding,
            controller,
        })
    }
    pub fn validate_for_pe(&self, pe: &super::ProcessingElement) -> gwr_engine::types::SimResult {
        self.binding.validate_for_pe(pe)
    }
    #[must_use]
    pub fn observation(&self) -> crate::memory::atomic::ControllerObservation {
        self.controller.observation()
    }
    pub fn execute(&self) -> gwr_engine::types::SimResult {
        self.controller.verify_final_join()
    }
}
