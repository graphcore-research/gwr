// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use super::FlowDemand;

#[derive(Clone, Copy, Debug)]
pub enum Topology {
    Shared,
    Disjoint,
    Intersecting,
}

pub struct Case {
    pub capacities: Vec<f64>,
    pub flows: Vec<FlowDemand>,
}

pub fn make(
    count: usize,
    resources: usize,
    degree: usize,
    topology: Topology,
    grouped: bool,
) -> Case {
    assert!(resources > 0 && degree > 0 && degree <= resources);
    let capacities = (0..resources).map(|r| 16. + 8. * (r % 4) as f64).collect();
    let mut seed = 17_u64;
    let flows = (0..count)
        .map(|i| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let indices: Vec<_> = match topology {
                Topology::Shared => vec![0],
                // Disjoint resource groups; flows in the same group still
                // contend.
                Topology::Disjoint => vec![i % resources],
                Topology::Intersecting => (0..degree)
                    .map(|j| (seed as usize % resources + j) % resources)
                    .collect(),
            };
            FlowDemand {
                id: i as u64,
                source: if grouped { (i / 8) as u64 } else { i as u64 },
                transfer: if grouped { (i / 2) as u64 } else { i as u64 },
                remaining_payload: 64 * (1 + i % 3) as u128,
                resource_coefficients: indices
                    .into_iter()
                    .enumerate()
                    .map(|(j, r)| (r, [0.125, 1., 1.125][(i + j) % 3]))
                    .collect(),
            }
        })
        .collect();
    Case { capacities, flows }
}
