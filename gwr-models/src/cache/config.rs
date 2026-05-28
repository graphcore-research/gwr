// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use crate::memory::memory_map::{DeviceId, MemoryMap};

#[derive(Clone)]
pub struct CacheConfig {
    pub line_size_bytes: usize,
    pub bw_bytes_per_cycle: usize,
    pub num_sets: usize,
    pub num_ways: usize,
    pub delay_ticks: usize,
    pub device_id: DeviceId,
    pub memory_map: Rc<MemoryMap>,
    pub coherency_manager_memory_map: Option<Rc<MemoryMap>>,
}

impl CacheConfig {
    #[must_use]
    pub fn new(
        device_id: DeviceId,
        line_size_bytes: usize,
        bw_bytes_per_cycle: usize,
        num_sets: usize,
        num_ways: usize,
        delay_ticks: usize,
        memory_map: &Rc<MemoryMap>,
    ) -> Self {
        Self {
            line_size_bytes,
            bw_bytes_per_cycle,
            num_sets,
            num_ways,
            delay_ticks,
            device_id,
            memory_map: memory_map.clone(),
            coherency_manager_memory_map: None,
        }
    }

    #[must_use]
    pub fn with_coherency_manager_memory_map(mut self, memory_map: &Rc<MemoryMap>) -> Self {
        self.coherency_manager_memory_map = Some(memory_map.clone());
        self
    }
}
