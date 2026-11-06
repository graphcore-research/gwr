// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use futures::StreamExt;
use futures::channel::oneshot::Sender;
use futures::future::LocalBoxFuture;
use futures::stream::FuturesUnordered;
use gwr_engine::sim_error;
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_track::entity::EntityGroup;

use super::TaskRunner;
use super::helpers::build_assignment_dependencies;
use crate::processing_element::program::analysis::access::collect_initial_input_reads;
use crate::processing_element::program::analysis::tile_storage::{TileStorage, build_tile_storage};
use crate::processing_element::program::plan::LocalAllocation;
use crate::processing_element::program::prepared::{GroupPlan, Schedule};
use crate::processing_element::program::tile::RegionTile;
use crate::processing_element::task::PreparedTask;
use crate::processing_element::tensors::TensorView;

impl TaskRunner {
    pub(super) async fn run_program_tile(
        &self,
        task: &PreparedTask,
        plan: GroupPlan<'_>,
        tile: RegionTile,
        admitted: Sender<()>,
    ) -> SimResult {
        let group = self.activity_lanes.create_group(&format!(
            "{} tile {:?}",
            task.id(),
            tile.output_intervals
        ));
        let storage = build_tile_storage(plan, &tile, self.sram_bytes)?;
        let _reservation = self.sram.reserve(storage.arena).await?;
        {
            let mut stats = self.stats.borrow_mut();
            stats.peak_sram_bytes = stats.peak_sram_bytes.max(self.sram.used());
        }
        let _ = admitted.send(());
        self.run_tile_actions(task, plan, tile, group, storage)
            .await
    }

    /// Run an admitted tile group using the same readiness and slot-reuse
    /// dependencies as a standalone tile. The enclosing scope owns admission.
    pub(super) async fn run_tile_actions(
        &self,
        task: &PreparedTask,
        plan: GroupPlan<'_>,
        tile: RegionTile,
        group: Rc<EntityGroup>,
        storage: TileStorage<'_>,
    ) -> SimResult {
        let TileStorage {
            outputs,
            local_allocation,
            ..
        } = storage;
        let mut active = self.start_tile_input_reads(task, plan, &tile, &group);
        let mut progress = TileProgress::new(plan, &local_allocation)?;
        let tile_ref = &tile;
        let group_ref = &group;
        while progress.has_pending_work() || !active.is_empty() {
            for index in progress.take_ready_assignments() {
                active.push(Box::pin(async move {
                    self.run_program_assignment(task, plan, tile_ref, group_ref, index)
                        .await?;
                    Ok(TileCompletion::Assignment(index))
                }));
            }
            for index in progress.take_ready_stores() {
                let name = outputs[index].0;
                let view = &outputs[index].1;
                active.push(Box::pin(async move {
                    self.store_tile_output(view, &format!("{} {name} write", task.id()), group_ref)
                        .await?;
                    Ok(TileCompletion::Store)
                }));
            }
            let Some(result) = active.next().await else {
                return sim_error!("tensor-program assignment dependencies cannot be scheduled");
            };
            progress.record_completion(result?);
        }
        Ok(())
    }

    fn start_tile_input_reads<'a>(
        &'a self,
        task: &'a PreparedTask,
        plan: GroupPlan<'a>,
        tile: &'a RegionTile,
        group: &'a Rc<EntityGroup>,
    ) -> TileActions<'a> {
        let active = TileActions::new();
        for read in collect_initial_input_reads(plan) {
            active.push(Box::pin(async move {
                self.load_input(
                    read.plan_access(tile, plan.region(), &BTreeMap::new())?,
                    &format!("{} {} read", task.id(), read.name),
                    group,
                )
                .await?;
                Ok(TileCompletion::Load(read.name.to_owned()))
            }));
        }
        active
    }

    async fn store_tile_output(
        &self,
        view: &TensorView,
        activity_name: &str,
        group: &Rc<EntityGroup>,
    ) -> SimResult {
        self.access_ranges(
            AccessType::WriteNonPostedRequest,
            view.address_ranges(),
            view.layout().num_access_bytes(),
            &self.activity_lanes.lsu_write,
            activity_name,
            group,
        )
        .await?;
        self.record_write(view)
    }
}

type TileActions<'a> = FuturesUnordered<LocalBoxFuture<'a, Result<TileCompletion, SimError>>>;

struct TileProgress {
    dependencies: BTreeMap<usize, BTreeSet<usize>>,
    input_dependencies: Vec<BTreeSet<String>>,
    pending: Vec<usize>,
    completed: BTreeSet<usize>,
    loaded: BTreeSet<String>,
    pending_stores: BTreeSet<usize>,
    output_producers: Vec<usize>,
}

impl TileProgress {
    fn new(plan: GroupPlan<'_>, allocation: &LocalAllocation) -> Result<Self, SimError> {
        let output_producers = plan
            .outputs()
            .map(|(name, _)| plan.region().producers[name])
            .collect::<Vec<_>>();
        let initial_inputs = collect_initial_input_reads(plan)
            .into_iter()
            .map(|read| read.name.to_owned())
            .collect::<BTreeSet<_>>();
        let input_dependencies = plan
            .region()
            .program
            .assignments
            .iter()
            .map(|assignment| {
                assignment
                    .expr
                    .collect_read_tensor_names()
                    .intersection(&initial_inputs)
                    .cloned()
                    .collect()
            })
            .collect();
        let mut pending = plan.assignments.to_vec();
        // Flat tiles historically issue ready assignments in source order;
        // scoped groups issue in their planned topological group order.
        if matches!(plan.program.schedule, Schedule::Flat(_)) {
            pending.sort_unstable();
        }
        Ok(Self {
            dependencies: build_assignment_dependencies(plan, allocation)?,
            input_dependencies,
            pending,
            completed: BTreeSet::new(),
            loaded: BTreeSet::new(),
            pending_stores: (0..output_producers.len()).collect(),
            output_producers,
        })
    }

    fn has_pending_work(&self) -> bool {
        !self.pending.is_empty() || !self.pending_stores.is_empty()
    }

    fn take_ready_assignments(&mut self) -> Vec<usize> {
        let mut ready = Vec::new();
        self.pending.retain(|index| {
            let available = self.dependencies[index].is_subset(&self.completed)
                && self.input_dependencies[*index].is_subset(&self.loaded);
            if available {
                ready.push(*index);
            }
            !available
        });
        ready
    }

    fn take_ready_stores(&mut self) -> Vec<usize> {
        let ready = self
            .pending_stores
            .iter()
            .copied()
            .filter(|index| self.completed.contains(&self.output_producers[*index]))
            .collect::<Vec<_>>();
        for index in &ready {
            self.pending_stores.remove(index);
        }
        ready
    }

    fn record_completion(&mut self, completion: TileCompletion) {
        match completion {
            TileCompletion::Load(name) => {
                self.loaded.insert(name);
            }
            TileCompletion::Assignment(index) => {
                self.completed.insert(index);
            }
            TileCompletion::Store => {}
        }
    }
}

enum TileCompletion {
    Load(String),
    Assignment(usize),
    Store,
}
