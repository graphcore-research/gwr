// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::port::PortStateResult;
use gwr_engine::types::SimResult;

use crate::memory::memory_access::MemoryAccess;

/// Deferred access to a detailed duplex port. Merely obtaining this handle does
/// not claim the input port. Both callbacks run only during actual wiring.
pub struct MemoryEndpoint<'a> {
    receive: Box<dyn Fn() -> PortStateResult<MemoryAccess> + 'a>,
    connect: Box<dyn Fn(PortStateResult<MemoryAccess>) -> SimResult + 'a>,
}

impl<'a> MemoryEndpoint<'a> {
    pub fn detailed(
        receive: impl Fn() -> PortStateResult<MemoryAccess> + 'a,
        connect: impl Fn(PortStateResult<MemoryAccess>) -> SimResult + 'a,
    ) -> Self {
        Self {
            receive: Box::new(receive),
            connect: Box::new(connect),
        }
    }

    /// Connect both directions using existing detailed-port semantics. Reject
    /// other protocols before invoking callbacks. A detailed port failure may
    /// occur after partial wiring; callers must discard that construction, not
    /// retry it as though port connection were transactional.
    pub fn connect_detailed(&self, other: &MemoryEndpoint<'_>) -> SimResult {
        (self.connect)((other.receive)())?;
        (other.connect)((self.receive)())
    }
}
