// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Priority Round Robin arbitration policy

use std::collections::BTreeMap;
use std::rc::Rc;

use tramway_engine::sim_error;
use tramway_engine::traits::SimObject;
use tramway_engine::types::SimError;
use tramway_track::entity::Entity;

use crate::arbiter::Arbitrate;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low = 0,
    Medium,
    High,
}

impl Default for Priority {
    fn default() -> Self {
        Self::Low
    }
}

struct PriorityLevel {
    current_candidate_index: usize,
    candidates: Vec<usize>,
}

pub struct PriorityRoundRobin<P>
where
    P: Copy + Default + Ord,
{
    priority_vec: Vec<P>,
    priority_map: Option<BTreeMap<P, PriorityLevel>>,
}

impl<P> PriorityRoundRobin<P>
where
    P: Copy + Default + Ord,
{
    #[must_use]
    pub fn new(num_inputs: usize) -> Self {
        let default_priority = P::default();
        let priority_vec = vec![default_priority; num_inputs];

        Self {
            priority_vec,
            priority_map: None,
        }
    }

    pub fn from_priorities(priority_vec: Vec<P>, num_inputs: usize) -> Result<Self, SimError> {
        if priority_vec.len() != num_inputs {
            return sim_error!("The number of priorities must be equal to the number of inputs");
        }

        Ok(Self {
            priority_vec,
            priority_map: None,
        })
    }

    pub fn set_priority(mut self, port_index: usize, new_priority: P) -> Self {
        self.priority_vec[port_index] = new_priority;
        self
    }

    fn create_map(&mut self) {
        let mut priority_map = BTreeMap::new();
        for (i, priority) in self.priority_vec.iter().enumerate() {
            priority_map
                .entry(*priority)
                .and_modify(|e: &mut PriorityLevel| {
                    e.candidates.push(i);
                })
                .or_insert(PriorityLevel {
                    current_candidate_index: 0,
                    candidates: Vec::from([i]),
                });
        }
        self.priority_map = Some(priority_map);
    }
}

impl<T, P> Arbitrate<T> for PriorityRoundRobin<P>
where
    T: SimObject,
    P: Copy + Default + Ord,
{
    fn arbitrate(
        &mut self,
        _entity: &Rc<Entity>,
        input_values: &mut [Option<T>],
    ) -> Option<(usize, T)> {
        if self.priority_map.is_none() {
            self.create_map();
        }
        let priority_map = self.priority_map.as_mut().unwrap();
        for (_, priority_level) in priority_map.iter_mut().rev() {
            let priority_vec = &priority_level.candidates;
            let num_inputs = priority_vec.len();
            for i in 0..num_inputs {
                let priority_index = (i + priority_level.current_candidate_index) % num_inputs;
                let input_index = priority_vec[priority_index];
                if let Some(value) = input_values[input_index].take() {
                    priority_level.current_candidate_index = (priority_index + 1) % num_inputs;
                    return Some((input_index, value));
                }
            }
        }
        None
    }
}
