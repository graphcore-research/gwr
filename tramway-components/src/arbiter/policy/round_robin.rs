// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Round Robin arbitration policy

use std::sync::Arc;

use tramway_engine::traits::SimObject;
use tramway_track::entity::Entity;

use crate::arbiter::Arbitrate;

pub struct RoundRobin {
    candidate: usize,
}

impl RoundRobin {
    #[must_use]
    pub fn new() -> Self {
        Self { candidate: 0 }
    }
}

impl Default for RoundRobin {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Arbitrate<T> for RoundRobin
where
    T: SimObject,
{
    fn arbitrate(&mut self, _entity: &Arc<Entity>, inputs: &mut [Option<T>]) -> Option<(usize, T)> {
        let num_inputs = inputs.len();
        for i in 0..num_inputs {
            let index = (i + self.candidate) % num_inputs;
            if let Some(value) = inputs[index].take() {
                self.candidate = (index + 1) % num_inputs;
                return Some((index, value));
            }
        }
        None
    }
}
