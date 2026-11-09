// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! PE dispatch, logical memory endpoints and reporting shared across
//! fidelities. Transport and task service policies remain
//! implementation-specific; attaching a dispatcher does not change how a PE
//! executes work.

use std::rc::Rc;

use gwr_track::entity::GetEntity;

use super::dispatch::Dispatch;
use super::{MachineOpCounts, ProcessingElement};
use crate::model::MemoryEndpoint;

pub trait ProcessingElementModel: GetEntity {
    /// Validate task support and service without starting execution.
    fn preflight_task(&self, task: &super::task::Task) -> gwr_engine::types::SimResult;
    /// Completed physical memory payload, including partition rereads.
    fn memory_traffic_bytes(&self) -> (usize, usize);
    /// Peak local storage observed by the model.
    fn peak_sram_bytes(&self) -> usize;

    fn endpoint(&self) -> MemoryEndpoint<'_>;
    fn dump_stats(&self, time_now_ns: f64);
    fn set_dispatcher(&self, dispatcher: &Rc<dyn Dispatch>);
    fn machine_ops(&self) -> MachineOpCounts;
    fn total_graph_nodes(&self) -> usize;
    fn bytes_consumed(&self) -> usize;
    fn bytes_produced(&self) -> usize;
}

impl ProcessingElementModel for ProcessingElement {
    fn preflight_task(&self, task: &super::task::Task) -> gwr_engine::types::SimResult {
        use super::task::Task;
        match task {
            Task::ProgramTask { config } => self.preflight_program_task(config),
            Task::TransferTask { config } => self.preflight_transfer_task(config),
            Task::AtomicEffect { config } => self.preflight_atomic_task(config),
            Task::AtomicControlJoin { config } => config.validate_for_pe(self),
            Task::StreamedMatrix { config } => config.validate_for_pe(self),
            Task::SyncTask { .. } => Ok(()),
        }
    }
    fn memory_traffic_bytes(&self) -> (usize, usize) {
        ProcessingElement::memory_traffic_bytes(self)
    }
    fn peak_sram_bytes(&self) -> usize {
        ProcessingElement::peak_sram_bytes(self)
    }

    fn endpoint(&self) -> MemoryEndpoint<'_> {
        MemoryEndpoint::detailed(|| self.port_rx(), |p| self.connect_port_tx(p))
    }
    fn dump_stats(&self, time_now_ns: f64) {
        ProcessingElement::dump_stats(self, time_now_ns);
    }
    fn set_dispatcher(&self, dispatcher: &Rc<dyn Dispatch>) {
        ProcessingElement::set_dispatcher(self, dispatcher);
    }

    fn machine_ops(&self) -> MachineOpCounts {
        ProcessingElement::machine_ops(self)
    }

    fn bytes_consumed(&self) -> usize {
        ProcessingElement::bytes_consumed(self)
    }
    fn bytes_produced(&self) -> usize {
        ProcessingElement::bytes_produced(self)
    }

    fn total_graph_nodes(&self) -> usize {
        ProcessingElement::total_graph_nodes(self)
    }
}
