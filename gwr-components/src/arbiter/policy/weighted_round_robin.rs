// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Weighted Round Robin policy

use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::traits::SimObject;
use gwr_engine::types::SimError;
use gwr_track::entity::Entity;
use gwr_track::trace;

use crate::arbiter::Arbitrate;

pub struct WeightedRoundRobin {
    candidate: usize,
    grants: Vec<usize>,
    weights: Vec<usize>,
}

impl WeightedRoundRobin {
    pub fn new(weights: Vec<usize>, num_inputs: usize) -> Result<Self, SimError> {
        if weights.len() != num_inputs {
            return sim_error!("The number of weights must be equal to the number of inputs");
        }

        Ok(Self {
            candidate: 0,
            grants: vec![0; weights.len()],
            weights,
        })
    }
}

impl WeightedRoundRobin {
    pub fn state_str<T>(&self, input_values: &[Option<T>]) -> String
    where
        T: SimObject,
    {
        let mut s = String::new();
        s.push_str(format!("{}: ", self.candidate).as_str());
        for (i, grant) in self.grants.iter().enumerate() {
            let req = if input_values[i].is_some() { "r" } else { "-" };
            s.push_str(format!("{}/{}/{}, ", req, grant, self.weights[i]).as_str());
        }
        s
    }
}

impl<T> Arbitrate<T> for WeightedRoundRobin
where
    T: SimObject,
{
    fn arbitrate(
        &mut self,
        entity: &Rc<Entity>,
        input_values: &mut [Option<T>],
    ) -> Option<(usize, T)> {
        trace!(entity ; "wrr: arbitrate {}", self.state_str(input_values));

        let num_inputs = input_values.len();
        let mut selected_candidate = None;
        for i in 0..num_inputs {
            let index = (i + self.candidate) % num_inputs;
            if input_values[index].is_none() {
                continue;
            }
            if self.weights[index] > self.grants[index] {
                selected_candidate = Some(index);
                break;
            } else if selected_candidate.is_none() {
                selected_candidate = Some(index);
            }
        }
        if let Some(index) = selected_candidate {
            if self.weights[index] == self.grants[index] {
                self.grants[index] = 0;
            }
            self.grants[index] += 1;

            let value = input_values[index].take().unwrap();
            self.candidate = (index + 1) % num_inputs;
            Some((index, value))
        } else {
            None
        }
    }
}
