// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::address::{
    AddressRange, MemoryRegion, TensorLayout, build_regions, clipped_range, merge_ranges,
    range_union_length,
};
use super::state::{
    AppState, EntityKind, Filter, PatternError, PeMeasure, PeMode, RelationshipMeasure,
    RelationshipMode,
};
use crate::model::{
    LayerPeSummary, LayerSummary, MachineOpSummary, PeSummary, ReportData, TensorAccess,
    TensorPeTraffic, TensorSummary, TensorTransfer,
};

#[derive(Debug, Default)]
pub(crate) struct LayerAggregate {
    pub(crate) compute_nodes: u64,
    pub(crate) by_op: BTreeMap<String, u64>,
    pub(crate) machine_ops: MachineOpSummary,
    pub(crate) active_pes: BTreeSet<String>,
    pub(crate) tensor_count: u64,
    pub(crate) read_bytes: u64,
    pub(crate) write_bytes: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ContextSummary {
    pub(crate) tensor_indices: Vec<usize>,
    pub(crate) read_bytes: u64,
    pub(crate) write_bytes: u64,
    pub(crate) edges: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FilteredSummary {
    pub(crate) compute_nodes: u64,
    pub(crate) by_op: BTreeMap<String, u64>,
    pub(crate) machine_ops: MachineOpSummary,
    pub(crate) tensors: u64,
    pub(crate) read_bytes: u64,
    pub(crate) write_bytes: u64,
    pub(crate) edges: u64,
    pub(crate) active_pes: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct VisibleConnection {
    pub(crate) pe: String,
    pub(crate) bytes: u64,
    pub(crate) edges: u64,
}

#[derive(Debug, Default)]
pub(crate) struct TensorTraffic {
    pub(crate) reads: Vec<VisibleConnection>,
    pub(crate) writes: Vec<VisibleConnection>,
    pub(crate) read_bytes: u64,
    pub(crate) write_bytes: u64,
    pub(crate) edges: u64,
    pub(crate) read_ratio: f64,
    pub(crate) write_ratio: f64,
}

pub(crate) struct AppModel {
    pub(crate) data: ReportData,
    pub(crate) state: AppState,
    pe_indices: HashMap<String, usize>,
    tensor_indices: HashMap<String, usize>,
    layer_indices: HashMap<String, usize>,
    tensors_by_layer: Vec<Vec<usize>>,
    tensors_by_pe: Vec<Vec<usize>>,
    cache: RefCell<ModelCache>,
}

const MAX_PE_GRID_CELLS: usize = 10_000;

#[derive(Default)]
struct ModelCache {
    generation: u64,
    contexts: HashMap<ContextKey, ContextSummary>,
    summary: Option<FilteredSummary>,
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct ContextKey {
    layer: Option<String>,
    pe: Option<String>,
}

impl AppModel {
    pub(crate) fn new(data: ReportData) -> Self {
        let layer_names = data
            .layers
            .iter()
            .map(|layer| layer.name.clone())
            .collect::<Vec<_>>();
        let pe_names = data
            .pes
            .iter()
            .map(|pe| pe.name.clone())
            .collect::<Vec<_>>();
        let memory_names = data
            .memory
            .platform_memories
            .iter()
            .map(|memory| memory.name.clone())
            .collect::<Vec<_>>();
        let tensor_names = data
            .tensors
            .iter()
            .map(|tensor| tensor.id.clone())
            .collect::<Vec<_>>();
        let selected_pe = data
            .pes
            .iter()
            .find(|pe| pe.total_nodes > 0)
            .or_else(|| data.pes.first())
            .map(|pe| pe.name.clone());
        let state = AppState {
            layers: Filter::new(layer_names),
            pes: Filter::new(pe_names),
            memories: Filter::new(memory_names),
            tensors: Filter::new(tensor_names),
            selected_layer: data.layers.first().map(|layer| layer.name.clone()),
            selected_pe,
            selected_memory: data
                .memory
                .platform_memories
                .first()
                .map(|memory| memory.name.clone()),
            selected_tensor: data.tensors.first().map(|tensor| tensor.id.clone()),
            pe_mode: PeMode::Grid,
            pe_measure: PeMeasure::MachineOps,
            relationship_mode: RelationshipMode::Compute,
            relationship_measure: RelationshipMeasure::MachineOps,
            relationship_strength: 85,
            skip_memory_gaps: true,
            generation: 0,
        };
        let pe_indices = index_names(data.pes.iter().map(|pe| pe.name.as_str()));
        let tensor_indices = index_names(data.tensors.iter().map(|tensor| tensor.id.as_str()));
        let layer_indices = index_names(data.layers.iter().map(|layer| layer.name.as_str()));
        let (tensors_by_layer, tensors_by_pe) =
            build_tensor_context_indices(&data.tensors, &layer_indices, &pe_indices);
        let mut model = Self {
            data,
            state,
            pe_indices,
            tensor_indices,
            layer_indices,
            tensors_by_layer,
            tensors_by_pe,
            cache: RefCell::new(ModelCache::default()),
        };
        model.set_pe_mode(PeMode::Grid);
        model
    }

    pub(crate) fn pe(&self, name: &str) -> Option<&PeSummary> {
        self.pe_indices
            .get(name)
            .map(|index| &self.data.pes[*index])
    }

    pub(crate) fn tensor(&self, id: &str) -> Option<&TensorSummary> {
        self.tensor_indices
            .get(id)
            .map(|index| &self.data.tensors[*index])
    }

    pub(crate) fn tensor_index(&self, id: &str) -> Option<usize> {
        self.tensor_indices.get(id).copied()
    }

    pub(crate) fn attach_tensors(
        &mut self,
        tensors: Vec<TensorSummary>,
    ) -> Result<(), PatternError> {
        let tensor_names = tensors
            .iter()
            .map(|tensor| tensor.id.clone())
            .collect::<Vec<_>>();
        self.tensor_indices = index_names(tensors.iter().map(|tensor| tensor.id.as_str()));
        (self.tensors_by_layer, self.tensors_by_pe) =
            build_tensor_context_indices(&tensors, &self.layer_indices, &self.pe_indices);
        self.state.tensors.replace_values(tensor_names)?;
        self.data.tensors = tensors;
        self.state.filters_changed();
        Ok(())
    }

    pub(crate) fn filter_value_count(&self, kind: EntityKind) -> u64 {
        if kind == EntityKind::Tensor && self.data.tensors.is_empty() {
            return self.data.summary.tensor_nodes;
        }
        u64::try_from(self.state.filter(kind).values().len())
            .expect("Wasm collection lengths fit in u64")
    }

    pub(crate) fn layer(&self, name: &str) -> Option<&LayerSummary> {
        self.layer_indices
            .get(name)
            .map(|index| &self.data.layers[*index])
    }

    pub(crate) fn selected_layer(&self) -> Option<&LayerSummary> {
        self.state
            .selected_layer
            .as_deref()
            .and_then(|name| self.layer(name))
    }

    pub(crate) fn selected_pe(&self) -> Option<&PeSummary> {
        self.state
            .selected_pe
            .as_deref()
            .and_then(|name| self.pe(name))
    }

    pub(crate) fn selected_tensor(&self) -> Option<&TensorSummary> {
        self.state
            .selected_tensor
            .as_deref()
            .and_then(|id| self.tensor(id))
    }

    pub(crate) fn filtered_layers(&self) -> impl Iterator<Item = &LayerSummary> {
        self.data
            .layers
            .iter()
            .filter(|layer| self.state.layers.is_selected(&layer.name))
    }

    pub(crate) fn filtered_tensors(&self) -> Vec<&TensorSummary> {
        self.context(None, None)
            .tensor_indices
            .into_iter()
            .map(|index| &self.data.tensors[index])
            .collect()
    }

    pub(crate) fn compute_population(&self) -> Vec<&PeSummary> {
        self.data
            .pes
            .iter()
            .filter(|pe| {
                (pe.present_in_platform || pe.present_in_timetable)
                    && self.state.pes.is_selected(&pe.name)
            })
            .collect()
    }

    pub(crate) fn layer_aggregate(&self, layer: &LayerSummary) -> LayerAggregate {
        if self.state.pes.is_all() {
            return LayerAggregate {
                compute_nodes: layer.compute_nodes,
                by_op: layer.by_op.clone(),
                machine_ops: layer.machine_ops.clone(),
                active_pes: layer
                    .pes
                    .iter()
                    .filter(|pe| pe.compute_nodes > 0)
                    .map(|pe| pe.name.clone())
                    .collect(),
                tensor_count: layer.tensor_count,
                read_bytes: layer.tensor_read_bytes,
                write_bytes: layer.tensor_write_bytes,
            };
        }
        layer
            .pes
            .iter()
            .filter(|pe| self.state.pes.is_selected(&pe.name))
            .fold(LayerAggregate::default(), add_layer_pe)
    }

    pub(crate) fn filtered_summary(&self) -> FilteredSummary {
        self.prepare_cache();
        if let Some(summary) = self.cache.borrow().summary.clone() {
            return summary;
        }
        if self.filters_are_all() {
            let summary = FilteredSummary {
                compute_nodes: self.data.summary.compute_nodes,
                by_op: sum_ops(self.data.layers.iter().map(|layer| &layer.by_op)),
                machine_ops: sum_machine_ops(
                    self.data.layers.iter().map(|layer| &layer.machine_ops),
                ),
                tensors: self.data.summary.tensor_nodes,
                read_bytes: self.data.summary.total_tensor_read_bytes,
                write_bytes: self.data.summary.total_tensor_write_bytes,
                edges: self.data.summary.data_edges,
                active_pes: self.data.summary.active_pes,
            };
            self.cache.borrow_mut().summary = Some(summary.clone());
            return summary;
        }
        let mut result = FilteredSummary::default();
        let mut active_pes = BTreeSet::new();
        for layer in self.filtered_layers() {
            let aggregate = self.layer_aggregate(layer);
            result.compute_nodes += aggregate.compute_nodes;
            add_ops(&mut result.by_op, &aggregate.by_op);
            add_machine_ops(&mut result.machine_ops, &aggregate.machine_ops);
            active_pes.extend(aggregate.active_pes);
        }
        let context = self.context(None, None);
        result.tensors = u64::try_from(context.tensor_indices.len())
            .expect("Wasm collection lengths fit in u64");
        result.read_bytes = context.read_bytes;
        result.write_bytes = context.write_bytes;
        result.edges = context.edges;
        result.active_pes = if self.state.layers.is_all() {
            self.data
                .pes
                .iter()
                .filter(|pe| {
                    pe.present_in_timetable
                        && pe.total_nodes > 0
                        && self.state.pes.is_selected(&pe.name)
                })
                .count()
                .try_into()
                .expect("Wasm collection lengths fit in u64")
        } else {
            active_pes
                .len()
                .try_into()
                .expect("Wasm collection lengths fit in u64")
        };
        self.cache.borrow_mut().summary = Some(result.clone());
        result
    }

    pub(crate) fn context(
        &self,
        exact_layer: Option<&str>,
        exact_pe: Option<&str>,
    ) -> ContextSummary {
        self.prepare_cache();
        let key = ContextKey {
            layer: exact_layer.map(str::to_string),
            pe: exact_pe.map(str::to_string),
        };
        if let Some(context) = self.cache.borrow().contexts.get(&key).cloned() {
            return context;
        }
        let traffic_unfiltered = exact_layer.is_none()
            && exact_pe.is_none()
            && self.state.layers.is_all()
            && self.state.pes.is_all();
        let mut summary = ContextSummary::default();
        match self.context_candidates(exact_layer, exact_pe) {
            TensorCandidates::All => {
                for index in 0..self.data.tensors.len() {
                    self.add_tensor_to_context(
                        &mut summary,
                        index,
                        exact_layer,
                        exact_pe,
                        traffic_unfiltered,
                    );
                }
            }
            TensorCandidates::Indexed(indices) => {
                for index in indices {
                    self.add_tensor_to_context(
                        &mut summary,
                        *index,
                        exact_layer,
                        exact_pe,
                        traffic_unfiltered,
                    );
                }
            }
        }
        self.cache
            .borrow_mut()
            .contexts
            .insert(key, summary.clone());
        summary
    }

    fn context_candidates<'a>(
        &'a self,
        exact_layer: Option<&str>,
        exact_pe: Option<&str>,
    ) -> TensorCandidates<'a> {
        let by_layer = exact_layer.map(|layer| {
            self.layer_indices
                .get(layer)
                .map_or(&[][..], |index| self.tensors_by_layer[*index].as_slice())
        });
        let by_pe = exact_pe.map(|pe| {
            self.pe_indices
                .get(pe)
                .map_or(&[][..], |index| self.tensors_by_pe[*index].as_slice())
        });
        match (by_layer, by_pe) {
            (None, None) => TensorCandidates::All,
            (Some(indices), None) | (None, Some(indices)) => TensorCandidates::Indexed(indices),
            (Some(layers), Some(pes)) => TensorCandidates::Indexed(if layers.len() <= pes.len() {
                layers
            } else {
                pes
            }),
        }
    }

    fn add_tensor_to_context(
        &self,
        summary: &mut ContextSummary,
        index: usize,
        exact_layer: Option<&str>,
        exact_pe: Option<&str>,
        traffic_unfiltered: bool,
    ) {
        let tensor = &self.data.tensors[index];
        if !self.state.tensors.is_selected(&tensor.id)
            || self.tensor_memory_overlap_bytes(tensor) == 0
        {
            return;
        }
        let traffic = self.tensor_traffic_for(tensor, exact_layer, exact_pe, None);
        if traffic_unfiltered
            || traffic.edges > 0
            || traffic.read_bytes > 0
            || traffic.write_bytes > 0
        {
            summary.tensor_indices.push(index);
            summary.read_bytes += traffic.read_bytes;
            summary.write_bytes += traffic.write_bytes;
            summary.edges += traffic.edges;
        }
    }

    pub(crate) fn clear_cache(&self) {
        let mut cache = self.cache.borrow_mut();
        cache.contexts.clear();
        cache.summary = None;
    }

    pub(crate) fn tensor_traffic(&self, tensor: &TensorSummary) -> TensorTraffic {
        self.tensor_traffic_for(tensor, None, None, None)
    }

    fn tensor_memory_overlap_bytes(&self, tensor: &TensorSummary) -> u64 {
        let tensor_bytes = tensor.num_bytes.max(1);
        let Some(memory_ranges) = self.selected_memory_ranges(None) else {
            return tensor_bytes;
        };
        let tensor_range = AddressRange::new(tensor.addr, tensor_bytes);
        let overlaps = memory_ranges
            .into_iter()
            .filter_map(|memory| tensor_range.intersection(memory));
        u64::try_from(range_union_length(overlaps).min(u128::from(tensor_bytes)))
            .expect("the overlap cannot exceed the tensor's u64 byte count")
    }

    pub(crate) fn machine_ops_for_pe(&self, pe: &PeSummary) -> MachineOpSummary {
        if self.state.layers.is_all() {
            return pe.machine_ops.clone();
        }
        sum_machine_ops(
            self.state
                .layers
                .selected_values()
                .filter_map(|layer| pe.machine_ops_by_layer.get(layer)),
        )
    }

    pub(crate) fn compute_nodes_for_pe(&self, pe_name: &str) -> (u64, BTreeMap<String, u64>) {
        let mut nodes = 0;
        let mut by_op = BTreeMap::new();
        for layer in self.filtered_layers() {
            if let Some(layer_pe) = layer.pes.iter().find(|candidate| candidate.name == pe_name) {
                nodes += layer_pe.compute_nodes;
                add_ops(&mut by_op, &layer_pe.by_op);
            }
        }
        (nodes, by_op)
    }

    fn dimensions(&self) -> (u64, u64) {
        let pe_rows = self
            .data
            .pes
            .iter()
            .map(|pe| pe.row.saturating_add(1))
            .max()
            .unwrap_or(1);
        let pe_cols = self
            .data
            .pes
            .iter()
            .map(|pe| pe.col.saturating_add(1))
            .max()
            .unwrap_or(1);
        let platform_rows = self
            .data
            .platform
            .as_ref()
            .map_or(0, |platform| platform.rows);
        let platform_cols = self
            .data
            .platform
            .as_ref()
            .map_or(0, |platform| platform.cols);
        let fabric_rows = self
            .data
            .platform
            .iter()
            .flat_map(|platform| &platform.fabrics)
            .map(|fabric| fabric.rows)
            .max()
            .unwrap_or(0);
        let fabric_cols = self
            .data
            .platform
            .iter()
            .flat_map(|platform| &platform.fabrics)
            .map(|fabric| fabric.cols)
            .max()
            .unwrap_or(0);
        (
            pe_rows.max(platform_rows).max(fabric_rows).max(1),
            pe_cols.max(platform_cols).max(fabric_cols).max(1),
        )
    }

    pub(crate) fn grid_dimensions(&self) -> Option<(usize, usize)> {
        let (rows, cols) = self.dimensions();
        grid_is_safe(rows, cols).then(|| {
            (
                usize::try_from(rows).expect("safe PE grid row count fits in usize"),
                usize::try_from(cols).expect("safe PE grid column count fits in usize"),
            )
        })
    }

    pub(crate) fn set_pe_mode(&mut self, requested: PeMode) {
        self.state.pe_mode = if requested == PeMode::Grid && self.grid_dimensions().is_none() {
            PeMode::Chart
        } else {
            requested
        };
    }

    pub(crate) fn memory_regions(&self) -> Vec<MemoryRegion> {
        let tensors = self.filtered_tensors();
        let mut layouts = Vec::new();
        for tensor in tensors {
            let tensor_index = self.tensor_indices[&tensor.id];
            if self.state.memories.is_all() {
                layouts.push(TensorLayout {
                    tensor_index,
                    address: tensor.addr,
                    bytes: tensor.num_bytes.max(1),
                });
                continue;
            }
            for memory in self
                .data
                .memory
                .platform_memories
                .iter()
                .filter(|memory| self.state.memories.is_selected(&memory.name))
            {
                if let Some((address, bytes)) = clipped_range(
                    tensor.addr,
                    tensor.num_bytes.max(1),
                    memory.base_addr,
                    memory.capacity_bytes,
                ) {
                    layouts.push(TensorLayout {
                        tensor_index,
                        address,
                        bytes,
                    });
                }
            }
        }
        build_regions(layouts, &self.data.tensors, self.state.skip_memory_gaps)
    }

    pub(crate) fn all_memory_regions(&self) -> Vec<MemoryRegion> {
        let layouts = self
            .data
            .tensors
            .iter()
            .enumerate()
            .map(|(tensor_index, tensor)| TensorLayout {
                tensor_index,
                address: tensor.addr,
                bytes: tensor.num_bytes.max(1),
            })
            .collect::<Vec<_>>();
        build_regions(layouts, &self.data.tensors, true)
    }

    pub(crate) fn tensor_traffic_for(
        &self,
        tensor: &TensorSummary,
        exact_layer: Option<&str>,
        exact_pe: Option<&str>,
        exact_memory: Option<&str>,
    ) -> TensorTraffic {
        let memory_ranges = self.selected_memory_ranges(exact_memory);
        let writes = self.visible_connections(
            tensor.addr,
            &tensor.writes_by_pe,
            exact_layer,
            exact_pe,
            memory_ranges.as_deref(),
        );
        let reads = self.visible_connections(
            tensor.addr,
            &tensor.reads_by_pe,
            exact_layer,
            exact_pe,
            memory_ranges.as_deref(),
        );
        let write_bytes = writes.iter().map(|connection| connection.bytes).sum();
        let read_bytes = reads.iter().map(|connection| connection.bytes).sum();
        let edges = reads
            .iter()
            .chain(&writes)
            .map(|connection| connection.edges)
            .sum();
        let tensor_bytes = tensor.num_bytes.max(1) as f64;
        TensorTraffic {
            reads,
            writes,
            read_bytes,
            write_bytes,
            edges,
            read_ratio: read_bytes as f64 / tensor_bytes,
            write_ratio: write_bytes as f64 / tensor_bytes,
        }
    }

    fn visible_connections(
        &self,
        tensor_addr: u64,
        connections: &[TensorPeTraffic],
        exact_layer: Option<&str>,
        exact_pe: Option<&str>,
        memory_ranges: Option<&[AddressRange]>,
    ) -> Vec<VisibleConnection> {
        connections
            .iter()
            .filter(|connection| {
                exact_pe.map_or_else(
                    || self.state.pes.is_selected(&connection.pe),
                    |pe| connection.pe == pe,
                )
            })
            .filter_map(|connection| {
                let (bytes, edges) = connection_traffic(
                    connection,
                    tensor_addr,
                    exact_layer,
                    &self.state.layers,
                    memory_ranges,
                );
                (bytes > 0 || edges > 0).then(|| VisibleConnection {
                    pe: connection.pe.clone(),
                    bytes,
                    edges,
                })
            })
            .collect()
    }

    fn selected_memory_ranges(&self, exact_memory: Option<&str>) -> Option<Vec<AddressRange>> {
        if exact_memory.is_none() && self.state.memories.is_all() {
            return None;
        }
        Some(
            self.data
                .memory
                .platform_memories
                .iter()
                .filter(|memory| {
                    exact_memory.map_or_else(
                        || self.state.memories.is_selected(&memory.name),
                        |name| memory.name == name,
                    )
                })
                .map(|memory| AddressRange::new(memory.base_addr, memory.capacity_bytes))
                .collect(),
        )
    }

    fn filters_are_all(&self) -> bool {
        self.state.layers.is_all()
            && self.state.pes.is_all()
            && self.state.memories.is_all()
            && self.state.tensors.is_all()
    }

    fn prepare_cache(&self) {
        let mut cache = self.cache.borrow_mut();
        if cache.generation != self.state.generation {
            cache.generation = self.state.generation;
            cache.contexts.clear();
            cache.summary = None;
        }
    }
}

enum TensorCandidates<'a> {
    All,
    Indexed(&'a [usize]),
}

fn grid_is_safe(rows: u64, cols: u64) -> bool {
    rows > 0
        && cols > 0
        && rows
            .checked_mul(cols)
            .is_some_and(|cells| cells <= MAX_PE_GRID_CELLS as u64)
}

fn index_names<'a>(names: impl Iterator<Item = &'a str>) -> HashMap<String, usize> {
    names
        .enumerate()
        .map(|(index, name)| (name.to_string(), index))
        .collect()
}

fn build_tensor_context_indices(
    tensors: &[TensorSummary],
    layer_indices: &HashMap<String, usize>,
    pe_indices: &HashMap<String, usize>,
) -> (Vec<Vec<usize>>, Vec<Vec<usize>>) {
    let mut tensors_by_layer = vec![Vec::new(); layer_indices.len()];
    let mut tensors_by_pe = vec![Vec::new(); pe_indices.len()];
    for (tensor_index, tensor) in tensors.iter().enumerate() {
        let mut layers = BTreeSet::new();
        let mut pes = BTreeSet::new();
        for connection in tensor.writes_by_pe.iter().chain(&tensor.reads_by_pe) {
            if let Some(pe_index) = pe_indices.get(&connection.pe) {
                pes.insert(*pe_index);
            }
            layers.extend(
                connection
                    .by_layer
                    .keys()
                    .chain(
                        connection
                            .transfers
                            .iter()
                            .filter_map(|access| access.layer.as_ref()),
                    )
                    .filter_map(|layer| layer_indices.get(layer))
                    .copied(),
            );
        }
        for layer_index in layers {
            tensors_by_layer[layer_index].push(tensor_index);
        }
        for pe_index in pes {
            tensors_by_pe[pe_index].push(tensor_index);
        }
    }
    (tensors_by_layer, tensors_by_pe)
}

fn add_layer_pe(mut total: LayerAggregate, pe: &LayerPeSummary) -> LayerAggregate {
    total.compute_nodes += pe.compute_nodes;
    add_ops(&mut total.by_op, &pe.by_op);
    add_machine_ops(&mut total.machine_ops, &pe.machine_ops);
    total.tensor_count += pe.tensor_count;
    total.read_bytes += pe.tensor_read_bytes;
    total.write_bytes += pe.tensor_write_bytes;
    if pe.compute_nodes > 0 {
        total.active_pes.insert(pe.name.clone());
    }
    total
}

fn add_ops(total: &mut BTreeMap<String, u64>, values: &BTreeMap<String, u64>) {
    for (name, value) in values {
        *total.entry(name.clone()).or_default() += value;
    }
}

fn sum_ops<'a>(values: impl Iterator<Item = &'a BTreeMap<String, u64>>) -> BTreeMap<String, u64> {
    let mut total = BTreeMap::new();
    for value in values {
        add_ops(&mut total, value);
    }
    total
}

fn add_machine_ops(total: &mut MachineOpSummary, value: &MachineOpSummary) {
    total.total += value.total;
    total.adds += value.adds;
    total.muls += value.muls;
    total.compares += value.compares;
}

fn sum_machine_ops<'a>(values: impl Iterator<Item = &'a MachineOpSummary>) -> MachineOpSummary {
    let mut total = MachineOpSummary::default();
    for value in values {
        add_machine_ops(&mut total, value);
    }
    total
}

fn connection_traffic(
    connection: &TensorPeTraffic,
    tensor_addr: u64,
    exact_layer: Option<&str>,
    layers: &Filter,
    memory_ranges: Option<&[AddressRange]>,
) -> (u64, u64) {
    connection
        .transfers
        .iter()
        .filter(|access| transfer_matches_layer(access, exact_layer, layers))
        .fold((0_u64, 0_u64), |(bytes, edges), access| {
            let selected_bytes = selected_access_bytes(&access.access, tensor_addr, memory_ranges);
            if selected_bytes == 0 {
                (bytes, edges)
            } else {
                (bytes + selected_bytes, edges + 1)
            }
        })
}

pub(crate) fn transfer_matches_layer(
    access: &TensorTransfer,
    exact_layer: Option<&str>,
    layers: &Filter,
) -> bool {
    exact_layer.map_or_else(
        || {
            layers.is_all()
                || access
                    .layer
                    .as_deref()
                    .is_some_and(|layer| layers.is_selected(layer))
        },
        |layer| access.layer.as_deref() == Some(layer),
    )
}

fn selected_access_bytes(
    access: &TensorAccess,
    tensor_addr: u64,
    memory_ranges: Option<&[AddressRange]>,
) -> u64 {
    let Some(memory_ranges) = memory_ranges else {
        return access.num_access_bytes;
    };
    let ranges = merge_ranges(memory_ranges.iter().copied());
    let bytes = ranges
        .iter()
        .map(|range| access.num_bytes_in(tensor_addr, *range))
        .sum::<u128>();
    u64::try_from(bytes).expect("report construction guarantees transfer byte counts fit in u64")
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "generator")]
    use std::path::Path;

    #[cfg(feature = "generator")]
    use gwr_platform::types::PlatformConfig;
    #[cfg(feature = "generator")]
    use gwr_timetable::timetable_file::TimetableFile;

    #[cfg(feature = "generator")]
    use super::AppModel;
    use super::{connection_traffic, grid_is_safe};
    #[cfg(feature = "generator")]
    use crate::analysis::build_report;
    use crate::model::{TensorAccess, TensorPeTraffic, TensorStride, TensorTransfer};
    use crate::web::address::AddressRange;
    use crate::web::state::Filter;

    #[test]
    fn attributes_exact_access_bytes_to_selected_layers_and_memories() {
        let connection = TensorPeTraffic {
            pe: "pe0".into(),
            bytes: 10,
            edge_count: 2,
            by_layer: std::collections::BTreeMap::new(),
            transfers: vec![
                TensorTransfer {
                    layer: Some("layer 1".into()),
                    access: TensorAccess {
                        first_element: 2,
                        elements_per_range: 2,
                        strides: vec![TensorStride {
                            count: 3,
                            stride_elements: 6,
                        }],
                        bits_per_element: 8,
                        num_access_bytes: 6,
                    },
                },
                TensorTransfer {
                    layer: Some("layer 2".into()),
                    access: TensorAccess {
                        first_element: 0,
                        elements_per_range: 4,
                        strides: Vec::new(),
                        bits_per_element: 8,
                        num_access_bytes: 4,
                    },
                },
            ],
        };
        let layers = Filter::new(vec!["layer 1".into(), "layer 2".into()]);
        let memories = [AddressRange::new(0, 4), AddressRange::new(8, 4)];

        assert_eq!(
            connection_traffic(&connection, 0, Some("layer 1"), &layers, Some(&memories)),
            (4, 1)
        );
    }

    #[test]
    fn rejects_unsafe_pe_grid_dimensions() {
        assert!(grid_is_safe(100, 100));
        assert!(!grid_is_safe(101, 100));
        assert!(!grid_is_safe(u64::MAX, 2));
    }

    #[test]
    #[cfg(feature = "generator")]
    fn counts_only_timetable_pes_when_filtering_the_full_layer_set() {
        let timetable =
            TimetableFile::from_file(Path::new("../gwr-timetable/examples/small.yaml")).unwrap();
        let platform: PlatformConfig = serde_yaml::from_str(
            &std::fs::read_to_string("../gwr-platform/examples/platform_4x4.yaml").unwrap(),
        )
        .unwrap();
        let graph = timetable.into_graph().unwrap();
        let data = build_report(
            &graph,
            Path::new("small.yaml"),
            Some((&platform, Path::new("platform.yaml"))),
            None,
        )
        .unwrap();
        let mut model = AppModel::new(data);

        model.state.pes.set_selected("pe_0_0", false);
        model.state.filters_changed();

        assert_eq!(model.filtered_summary().active_pes, 2);
    }

    #[test]
    #[cfg(feature = "generator")]
    fn indexed_contexts_match_full_tensor_scans() {
        let timetable =
            TimetableFile::from_file(Path::new("../gwr-timetable/examples/small.yaml")).unwrap();
        let graph = timetable.into_graph().unwrap();
        let data = build_report(&graph, Path::new("small.yaml"), None, None).unwrap();
        let model = AppModel::new(data);
        let layers = model
            .data
            .layers
            .iter()
            .map(|layer| layer.name.clone())
            .collect::<Vec<_>>();
        let pes = model
            .data
            .pes
            .iter()
            .map(|pe| pe.name.clone())
            .collect::<Vec<_>>();

        for (layer, pe) in layers
            .iter()
            .map(|layer| (Some(layer.as_str()), None))
            .chain(pes.iter().map(|pe| (None, Some(pe.as_str()))))
            .chain(layers.iter().flat_map(|layer| {
                pes.iter()
                    .map(move |pe| (Some(layer.as_str()), Some(pe.as_str())))
            }))
        {
            let mut expected = super::ContextSummary::default();
            for index in 0..model.data.tensors.len() {
                model.add_tensor_to_context(&mut expected, index, layer, pe, false);
            }
            assert_eq!(model.context(layer, pe), expected);
        }
    }
}
