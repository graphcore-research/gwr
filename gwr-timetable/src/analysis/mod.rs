// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Fast aggregate timetable analysis.

mod fast;
pub use fast::{ActivityEstimate, estimate_makespan};
pub mod roofline;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::fast::{ActivityEstimate, estimate_makespan};

    #[test]
    fn aggregate_estimate_combines_pe_work_critical_path_and_shared_resources() {
        let activities = vec![
            ActivityEstimate {
                pe: "pe0".into(),
                local_ticks: 4.0,
                engine_demands: BTreeMap::from([("pe0:matrix".into(), 4.0)]),
                predecessors: vec![],
                resources: BTreeMap::from([("hbm".into(), 4.0)]),
            },
            ActivityEstimate {
                pe: "pe1".into(),
                local_ticks: 6.0,
                engine_demands: BTreeMap::from([("pe1:vector".into(), 6.0)]),
                predecessors: vec![],
                resources: BTreeMap::from([("hbm".into(), 6.0)]),
            },
            ActivityEstimate {
                pe: "pe0".into(),
                local_ticks: 3.0,
                engine_demands: BTreeMap::from([("pe0:matrix".into(), 3.0)]),
                predecessors: vec![0],
                resources: BTreeMap::from([("hbm".into(), 3.0)]),
            },
        ];

        // PE work = max(7, 6), dependency path = 7, shared HBM demand = 13.
        assert_eq!(estimate_makespan(&activities).unwrap(), 13.0);
    }

    #[test]
    fn separate_engines_on_one_pe_are_aggregated_concurrently() {
        let activities = vec![
            ActivityEstimate {
                pe: "pe0".into(),
                local_ticks: 5.0,
                engine_demands: BTreeMap::from([("pe0:matrix".into(), 5.0)]),
                predecessors: vec![],
                resources: BTreeMap::new(),
            },
            ActivityEstimate {
                pe: "pe0".into(),
                local_ticks: 6.0,
                engine_demands: BTreeMap::from([("pe0:vector".into(), 6.0)]),
                predecessors: vec![],
                resources: BTreeMap::new(),
            },
        ];

        assert_eq!(estimate_makespan(&activities).unwrap(), 6.0);
    }
}
