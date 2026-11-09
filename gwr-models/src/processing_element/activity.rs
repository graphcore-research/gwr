// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

use gwr_track::entity::{Entity, EntityGroup, EntityLane};

use super::compute::ComputeModel;

pub(super) struct ProcessingElementActivityLanes {
    entity: Rc<Entity>,
    pub(super) compute: Vec<Rc<RefCell<ActivityLanes>>>,
    pub(super) compute_wait: Rc<RefCell<ActivityLanes>>,
    pub(super) lsu_read: Rc<RefCell<ActivityLanes>>,
    pub(super) lsu_write: Rc<RefCell<ActivityLanes>>,
    pub(super) local_payload: Rc<RefCell<ActivityLanes>>,
}

impl ProcessingElementActivityLanes {
    pub(super) fn new(entity: Rc<Entity>, model: &ComputeModel) -> Self {
        Self {
            entity: entity.clone(),
            compute: model
                .engine_names()
                .iter()
                .map(|name| {
                    Rc::new(RefCell::new(ActivityLanes::new(
                        entity.clone(),
                        &format!("lane::compute::{name}"),
                    )))
                })
                .collect(),
            compute_wait: Rc::new(RefCell::new(ActivityLanes::new(
                entity.clone(),
                "lane::compute_wait",
            ))),
            lsu_read: Rc::new(RefCell::new(ActivityLanes::new(
                entity.clone(),
                "lane::lsu_read",
            ))),
            local_payload: Rc::new(RefCell::new(ActivityLanes::new(
                entity.clone(),
                "lane::local_payload",
            ))),
            lsu_write: Rc::new(RefCell::new(ActivityLanes::new(entity, "lane::lsu_write"))),
        }
    }

    pub(super) fn create_group(&self, name: &str) -> Rc<EntityGroup> {
        Rc::new(EntityGroup::new(&self.entity, name))
    }
}

pub(crate) struct ActivityLanes {
    entity: Rc<Entity>,
    track_name: String,
    lanes: Vec<Lane>,
}

impl ActivityLanes {
    pub(super) fn new(entity: Rc<Entity>, track_name: &str) -> Self {
        Self {
            entity,
            track_name: track_name.to_string(),
            lanes: Vec::new(),
        }
    }

    pub(super) fn begin_in_group(
        lanes: &Rc<RefCell<Self>>,
        name: &str,
        group: &Rc<EntityGroup>,
    ) -> ActivityLaneGuard {
        let mut lanes_ref = lanes.borrow_mut();
        let lane_idx = match lanes_ref.lanes.iter().position(|lane| !lane.active) {
            Some(lane_idx) => lane_idx,
            None => lanes_ref.add_new_lane(),
        };

        let lane = &mut lanes_ref.lanes[lane_idx];
        lane.lane.begin_in_group(name, group);
        lane.active = true;

        ActivityLaneGuard {
            lanes: lanes.clone(),
            lane_idx,
            active: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn allocated_count_for_test(&self) -> usize {
        self.lanes.len()
    }

    #[cfg(test)]
    pub(crate) fn active_count_for_test(&self) -> usize {
        self.lanes.iter().filter(|lane| lane.active).count()
    }

    fn add_new_lane(&mut self) -> usize {
        let lane_idx = self.lanes.len();
        let lane = EntityLane::new(&self.entity, &format!("{}::{lane_idx}", self.track_name));
        self.lanes.push(Lane {
            lane,
            active: false,
        });
        lane_idx
    }

    fn end(&mut self, lane_idx: usize) {
        let lane = &mut self.lanes[lane_idx];
        lane.lane.end();
        lane.active = false;
    }
}

pub(super) struct ActivityLaneGuard {
    lanes: Rc<RefCell<ActivityLanes>>,
    lane_idx: usize,
    active: bool,
}

impl Drop for ActivityLaneGuard {
    fn drop(&mut self) {
        if self.active {
            self.lanes.borrow_mut().end(self.lane_idx);
            self.active = false;
        }
    }
}

struct Lane {
    lane: EntityLane,
    active: bool,
}
