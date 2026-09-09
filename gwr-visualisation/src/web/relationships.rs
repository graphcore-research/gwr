// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap, HashMap};

use super::address::AddressRange;
use super::logic::{AppModel, transfer_matches_layer};
use super::state::{RelationshipMeasure, RelationshipMode};
use crate::model::{LayerSummary, MemoryDeviceSummary, PeSummary, TensorSummary};

const MAX_SOURCES: usize = 500;
const MAX_EDGES: usize = 5_000;

type MemoryEdges = HashMap<String, HashMap<String, f64>>;

#[derive(Clone, Debug)]
pub(crate) struct RelationshipNode {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) group: String,
}

#[derive(Clone, Debug)]
pub(crate) struct RelationshipEdge {
    pub(crate) source: String,
    pub(crate) target: String,
    pub(crate) value: f64,
}

#[derive(Debug)]
pub(crate) struct RelationshipModel {
    pub(crate) sources: Vec<RelationshipNode>,
    pub(crate) targets: Vec<RelationshipNode>,
    pub(crate) edges: Vec<RelationshipEdge>,
    pub(crate) source_label: &'static str,
    pub(crate) target_label: &'static str,
    pub(crate) total: f64,
    pub(crate) matching_edges: usize,
    pub(crate) omitted_sources: usize,
    pub(crate) omitted_edges: usize,
}

pub(crate) fn build(model: &AppModel) -> RelationshipModel {
    let mode = model.state.relationship_mode;
    let memory_edges = collect_memory_edges(model, mode);
    let mut sources = sources(model, mode)
        .into_iter()
        .filter(|source| source.has_indexed_edges(model, &memory_edges))
        .collect::<Vec<_>>();
    let selected = selected_source(model, mode);
    if let Some(index) =
        selected.and_then(|selected| sources.iter().position(|source| source.id() == selected))
    {
        let source = sources.remove(index);
        sources.insert(0, source);
    }
    let matching_sources = sources.len();
    sources.truncate(MAX_SOURCES);
    let (window, edges, totals) = relationship_window(model, sources, selected, &memory_edges);
    let retained_source_ids = edges
        .iter()
        .map(|edge| edge.source.as_str())
        .collect::<BTreeSet<_>>();
    let mut source_nodes = window
        .into_iter()
        .filter(|source| retained_source_ids.contains(source.id()))
        .map(|source| source.node(model))
        .collect::<Vec<_>>();
    let target_ids = edges
        .iter()
        .map(|edge| edge.target.as_str())
        .collect::<BTreeSet<_>>();
    let mut target_nodes = target_ids
        .into_iter()
        .filter_map(|id| target_node(model, mode, id))
        .collect::<Vec<_>>();
    sort_nodes(model, mode, &mut source_nodes, &mut target_nodes);
    let (source_label, target_label) = labels(mode);
    let omitted_sources = matching_sources.saturating_sub(source_nodes.len());
    let omitted_edges = totals.edges.saturating_sub(edges.len());
    RelationshipModel {
        sources: source_nodes,
        targets: target_nodes,
        edges,
        source_label,
        target_label,
        total: totals.value,
        matching_edges: totals.edges,
        omitted_sources,
        omitted_edges,
    }
}

#[derive(Clone, Copy)]
enum Source<'a> {
    Compute(&'a LayerSummary),
    LayerMemory(&'a LayerSummary),
    PeMemory(&'a PeSummary),
    TensorMemory(&'a TensorSummary),
    TensorPe(&'a TensorSummary),
}

impl<'a> Source<'a> {
    fn id(self) -> &'a str {
        match self {
            Self::Compute(layer) | Self::LayerMemory(layer) => &layer.name,
            Self::PeMemory(pe) => &pe.name,
            Self::TensorMemory(tensor) | Self::TensorPe(tensor) => &tensor.id,
        }
    }

    fn node(self, model: &AppModel) -> RelationshipNode {
        match self {
            Self::Compute(layer) | Self::LayerMemory(layer) => {
                node(&layer.name, &layer.name, layer_band(&layer.name))
            }
            Self::PeMemory(pe) => node(&pe.name, &pe.name, format!("PE row {}", pe.row)),
            Self::TensorMemory(tensor) | Self::TensorPe(tensor) => tensor_node(model, tensor),
        }
    }

    fn has_indexed_edges(self, model: &AppModel, memory_edges: &MemoryEdges) -> bool {
        match self {
            Self::Compute(layer) => layer
                .pes
                .iter()
                .filter(|pe| model.state.pes.is_selected(&pe.name))
                .any(|pe| compute_edge_value(pe, &model.state.relationship_measure) > 0.0),
            Self::LayerMemory(layer) => memory_edges.contains_key(&layer.name),
            Self::PeMemory(pe) => memory_edges.contains_key(&pe.name),
            Self::TensorMemory(tensor) => memory_edges.contains_key(&tensor.id),
            Self::TensorPe(_) => true,
        }
    }

    fn visit_edges(
        self,
        model: &AppModel,
        memory_edges: &MemoryEdges,
        mut visit: impl FnMut(RelationshipEdge),
    ) {
        match self {
            Self::Compute(layer) => visit_compute_edges(model, layer, visit),
            Self::LayerMemory(layer) => {
                visit_memory_edges(&layer.name, memory_edges, &mut visit);
            }
            Self::PeMemory(pe) => {
                visit_memory_edges(&pe.name, memory_edges, &mut visit);
            }
            Self::TensorMemory(tensor) => {
                visit_memory_edges(&tensor.id, memory_edges, &mut visit);
            }
            Self::TensorPe(tensor) => visit_tensor_pe_edges(model, tensor, visit),
        }
    }
}

#[derive(Default)]
struct MatchTotals {
    edges: usize,
    value: f64,
}

fn sources(model: &AppModel, mode: RelationshipMode) -> Vec<Source<'_>> {
    match mode {
        RelationshipMode::Compute => model.filtered_layers().map(Source::Compute).collect(),
        RelationshipMode::LayerMemory => model.filtered_layers().map(Source::LayerMemory).collect(),
        RelationshipMode::PeMemory => {
            let mut pes = model.compute_population();
            pes.sort_by_key(|pe| (pe.row, pe.col, pe.name.as_str()));
            pes.into_iter().map(Source::PeMemory).collect()
        }
        RelationshipMode::TensorMemory => tensors_for_context(model, None, None)
            .into_iter()
            .map(Source::TensorMemory)
            .collect(),
        RelationshipMode::TensorPe => tensors_for_context(model, None, None)
            .into_iter()
            .map(Source::TensorPe)
            .collect(),
    }
}

fn selected_source(model: &AppModel, mode: RelationshipMode) -> Option<&str> {
    match mode {
        RelationshipMode::Compute | RelationshipMode::LayerMemory => {
            model.state.selected_layer.as_deref()
        }
        RelationshipMode::PeMemory => model.state.selected_pe.as_deref(),
        RelationshipMode::TensorMemory | RelationshipMode::TensorPe => {
            model.state.selected_tensor.as_deref()
        }
    }
}

fn relationship_window<'a>(
    model: &AppModel,
    sources: Vec<Source<'a>>,
    selected: Option<&str>,
    memory_edges: &MemoryEdges,
) -> (Vec<Source<'a>>, Vec<RelationshipEdge>, MatchTotals) {
    let mut window = Vec::with_capacity(MAX_SOURCES);
    let mut totals = MatchTotals::default();
    let mut retained = BinaryHeap::with_capacity(MAX_EDGES + 1);
    let mut selected_edge = None;
    for source in sources {
        let mut edge_count = 0;
        let is_selected = selected == Some(source.id());
        source.visit_edges(model, memory_edges, |edge| {
            edge_count += 1;
            totals.edges += 1;
            totals.value += edge.value;
            if is_selected
                && selected_edge
                    .as_ref()
                    .is_none_or(|current| edge_order(&edge, current) == Ordering::Less)
            {
                selected_edge = Some(edge.clone());
            }
            retain_edge(&mut retained, edge);
        });
        if edge_count == 0 {
            continue;
        }
        window.push(source);
    }
    if let Some(selected_edge) = selected_edge
        && !retained
            .iter()
            .any(|edge| edge.0.source == selected_edge.source)
    {
        if retained.len() == MAX_EDGES {
            retained.pop();
        }
        retained.push(RankedEdge(selected_edge));
    }
    let mut edges = retained.into_iter().map(|edge| edge.0).collect::<Vec<_>>();
    edges.sort_by(edge_order);
    (window, edges, totals)
}

#[derive(Debug)]
struct RankedEdge(RelationshipEdge);

impl PartialEq for RankedEdge {
    fn eq(&self, other: &Self) -> bool {
        edge_order(&self.0, &other.0) == Ordering::Equal
    }
}

impl Eq for RankedEdge {}

impl PartialOrd for RankedEdge {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedEdge {
    fn cmp(&self, other: &Self) -> Ordering {
        edge_order(&self.0, &other.0)
    }
}

fn retain_edge(retained: &mut BinaryHeap<RankedEdge>, edge: RelationshipEdge) {
    retained.push(RankedEdge(edge));
    if retained.len() > MAX_EDGES {
        retained.pop();
    }
}

fn edge_order(left: &RelationshipEdge, right: &RelationshipEdge) -> Ordering {
    right
        .value
        .total_cmp(&left.value)
        .then_with(|| left.source.cmp(&right.source))
        .then_with(|| left.target.cmp(&right.target))
}

fn visit_compute_edges(
    model: &AppModel,
    layer: &LayerSummary,
    mut visit: impl FnMut(RelationshipEdge),
) {
    for pe in layer
        .pes
        .iter()
        .filter(|pe| model.state.pes.is_selected(&pe.name))
    {
        let value = compute_edge_value(pe, &model.state.relationship_measure);
        if value > 0.0 {
            visit(RelationshipEdge {
                source: layer.name.clone(),
                target: pe.name.clone(),
                value,
            });
        }
    }
}

fn visit_tensor_pe_edges(
    model: &AppModel,
    tensor: &TensorSummary,
    mut visit: impl FnMut(RelationshipEdge),
) {
    let traffic = model.tensor_traffic(tensor);
    let connections = if model.state.relationship_measure == RelationshipMeasure::Read {
        &traffic.reads
    } else {
        &traffic.writes
    };
    for connection in connections {
        visit(RelationshipEdge {
            source: tensor.id.clone(),
            target: connection.pe.clone(),
            value: connection.bytes as f64,
        });
    }
}

fn collect_memory_edges(model: &AppModel, mode: RelationshipMode) -> MemoryEdges {
    if !mode.needs_platform() {
        return HashMap::new();
    }
    let memories = relationship_memories(model).collect::<Vec<_>>();
    let mut edges = MemoryEdges::new();
    for tensor in &model.data.tensors {
        if !model.state.tensors.is_selected(&tensor.id) {
            continue;
        }
        let connections = if model.state.relationship_measure == RelationshipMeasure::Read {
            &tensor.reads_by_pe
        } else {
            &tensor.writes_by_pe
        };
        for connection in connections {
            if !model.state.pes.is_selected(&connection.pe) {
                continue;
            }
            for transfer in &connection.transfers {
                if !transfer_matches_layer(transfer, None, &model.state.layers) {
                    continue;
                }
                let Some(source) =
                    memory_edge_source(mode, tensor, &connection.pe, transfer.layer.as_deref())
                else {
                    continue;
                };
                for memory in &memories {
                    let bytes = transfer.access.num_bytes_in(
                        tensor.addr,
                        AddressRange::new(memory.base_addr, memory.capacity_bytes),
                    );
                    if bytes > 0 {
                        *edges
                            .entry(source.to_string())
                            .or_default()
                            .entry(memory.name.clone())
                            .or_default() += bytes as f64;
                    }
                }
            }
        }
    }
    edges
}

fn memory_edge_source<'a>(
    mode: RelationshipMode,
    tensor: &'a TensorSummary,
    pe: &'a str,
    layer: Option<&'a str>,
) -> Option<&'a str> {
    match mode {
        RelationshipMode::LayerMemory => layer,
        RelationshipMode::PeMemory => Some(pe),
        RelationshipMode::TensorMemory => Some(&tensor.id),
        RelationshipMode::Compute | RelationshipMode::TensorPe => None,
    }
}

fn visit_memory_edges(
    source: &str,
    memory_edges: &MemoryEdges,
    visit: &mut impl FnMut(RelationshipEdge),
) {
    if let Some(edges) = memory_edges.get(source) {
        for (target, value) in edges {
            visit(RelationshipEdge {
                source: source.to_string(),
                target: target.clone(),
                value: *value,
            });
        }
    }
}

fn tensors_for_context<'a>(
    model: &'a AppModel,
    exact_layer: Option<&str>,
    exact_pe: Option<&str>,
) -> Vec<&'a TensorSummary> {
    model
        .context(exact_layer, exact_pe)
        .tensor_indices
        .into_iter()
        .map(|index| &model.data.tensors[index])
        .collect()
}

fn relationship_memories(model: &AppModel) -> impl Iterator<Item = &MemoryDeviceSummary> {
    model
        .data
        .memory
        .platform_memories
        .iter()
        .filter(|memory| model.state.memories.is_selected(&memory.name))
}

fn target_node(model: &AppModel, mode: RelationshipMode, id: &str) -> Option<RelationshipNode> {
    match mode {
        RelationshipMode::Compute | RelationshipMode::TensorPe => model
            .pe(id)
            .map(|pe| node(id, id, format!("PE row {}", pe.row))),
        RelationshipMode::LayerMemory
        | RelationshipMode::PeMemory
        | RelationshipMode::TensorMemory => {
            let memory = model
                .data
                .memory
                .platform_memories
                .iter()
                .find(|memory| memory.name == id)?;
            let index = model
                .data
                .memory
                .platform_memories
                .iter()
                .position(|candidate| candidate.name == memory.name)
                .unwrap_or(0);
            let start = index / 4 * 4;
            Some(node(
                id,
                id,
                format!("{} {start}-{}", memory.kind, start + 3),
            ))
        }
    }
}

fn sort_nodes(
    model: &AppModel,
    mode: RelationshipMode,
    sources: &mut [RelationshipNode],
    targets: &mut [RelationshipNode],
) {
    match mode {
        RelationshipMode::Compute | RelationshipMode::LayerMemory => {}
        RelationshipMode::PeMemory => sort_pes(model, sources),
        RelationshipMode::TensorMemory | RelationshipMode::TensorPe => {
            sort_tensors(model, sources);
        }
    }
    if matches!(mode, RelationshipMode::Compute | RelationshipMode::TensorPe) {
        sort_pes(model, targets);
    }
}

fn labels(mode: RelationshipMode) -> (&'static str, &'static str) {
    match mode {
        RelationshipMode::Compute => ("layers", "PEs"),
        RelationshipMode::LayerMemory => ("layers", "memories"),
        RelationshipMode::PeMemory => ("PEs", "memories"),
        RelationshipMode::TensorMemory => ("tensors", "memories"),
        RelationshipMode::TensorPe => ("tensors", "PEs"),
    }
}

fn compute_edge_value(pe: &crate::model::LayerPeSummary, measure: &RelationshipMeasure) -> f64 {
    match measure {
        RelationshipMeasure::ComputeNodes => pe.compute_nodes as f64,
        RelationshipMeasure::MachineOperation(name) if name == "adds" => pe.machine_ops.adds as f64,
        RelationshipMeasure::MachineOperation(name) if name == "muls" => pe.machine_ops.muls as f64,
        RelationshipMeasure::MachineOperation(name) if name == "compares" => {
            pe.machine_ops.compares as f64
        }
        RelationshipMeasure::MachineOps
        | RelationshipMeasure::MachineOperation(_)
        | RelationshipMeasure::Read
        | RelationshipMeasure::Write => pe.machine_ops.total as f64,
    }
}

fn tensor_node(model: &AppModel, tensor: &TensorSummary) -> RelationshipNode {
    let layer = first_tensor_layer(model, tensor).unwrap_or_else(|| "Unassigned tensors".into());
    node(&tensor.id, &tensor.id, layer)
}

fn first_tensor_layer(model: &AppModel, tensor: &TensorSummary) -> Option<String> {
    let order = |name: &String| {
        model
            .data
            .layers
            .iter()
            .position(|layer| &layer.name == name)
            .unwrap_or(usize::MAX)
    };
    let mut writes = connection_layers(model, &tensor.writes_by_pe);
    writes.sort_by_key(order);
    if let Some(layer) = writes.first() {
        return Some(layer.clone());
    }
    let mut reads = connection_layers(model, &tensor.reads_by_pe);
    reads.sort_by_key(order);
    reads.first().cloned()
}

fn connection_layers(
    model: &AppModel,
    connections: &[crate::model::TensorPeTraffic],
) -> Vec<String> {
    let mut values = connections
        .iter()
        .flat_map(|connection| connection.by_layer.keys())
        .filter(|layer| model.state.layers.is_selected(layer))
        .cloned()
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn sort_pes(model: &AppModel, nodes: &mut [RelationshipNode]) {
    nodes.sort_by_key(|node| {
        let pe = model.pe(&node.id);
        (
            pe.map_or(0, |pe| pe.row),
            pe.map_or(0, |pe| pe.col),
            node.id.clone(),
        )
    });
}

fn sort_tensors(model: &AppModel, nodes: &mut [RelationshipNode]) {
    nodes.sort_by_key(|node| {
        let group = model
            .data
            .layers
            .iter()
            .position(|layer| layer.name == node.group)
            .unwrap_or(usize::MAX);
        let address = model.tensor(&node.id).map_or(0, |tensor| tensor.addr);
        (group, address, node.id.clone())
    });
}

fn node(id: &str, label: &str, group: String) -> RelationshipNode {
    RelationshipNode {
        id: id.into(),
        label: label.into(),
        group,
    }
}

fn layer_band(name: &str) -> String {
    let digits = name
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    match digits.parse::<usize>() {
        Ok(number) if number > 0 => {
            let start = (number - 1) / 10 * 10 + 1;
            format!("Layers {start}-{}", start + 9)
        }
        _ if name == "pre-layer" => "Pre-layer".into(),
        _ => "Unassigned layers".into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{RelationshipEdge, build, collect_memory_edges, edge_order, layer_band};
    use crate::model::{
        LayerPeSummary, LayerSummary, MachineOpSummary, MemoryDeviceSummary, MemorySummary,
        PeSummary, ReportData, Summary, TensorAccess, TensorPeTraffic, TensorSummary,
        TensorTransfer,
    };
    use crate::web::logic::AppModel;
    use crate::web::state::{RelationshipMeasure, RelationshipMode};

    #[test]
    fn groups_numbered_layers_in_tens() {
        assert_eq!(layer_band("layer 13"), "Layers 11-20");
        assert_eq!(layer_band("pre-layer"), "Pre-layer");
    }

    #[test]
    fn ranks_larger_edges_first_then_uses_names() {
        let mut edges = [
            RelationshipEdge {
                source: "b".into(),
                target: "a".into(),
                value: 1.0,
            },
            RelationshipEdge {
                source: "a".into(),
                target: "b".into(),
                value: 2.0,
            },
            RelationshipEdge {
                source: "a".into(),
                target: "a".into(),
                value: 1.0,
            },
        ];
        edges.sort_by(edge_order);

        assert_eq!(edges[0].value, 2.0);
        assert_eq!(edges[1].source, "a");
        assert_eq!(edges[1].target, "a");
    }

    #[test]
    fn filters_sources_before_applying_the_source_limit() {
        let mut layers = (0..500)
            .map(|index| layer(index, Vec::new()))
            .collect::<Vec<_>>();
        layers.push(layer(500, vec![layer_pe("pe", 1)]));
        let model = AppModel::new(report(layers, vec![pe("pe", 0)]));

        let relation = build(&model);

        assert_eq!(relation.sources.len(), 1);
        assert_eq!(relation.sources[0].id, "layer 500");
        assert_eq!(relation.omitted_sources, 0);
    }

    #[test]
    fn bounds_relationship_sources_edges_and_rendered_nodes() {
        let pes = (0..20)
            .map(|index| pe(&format!("pe {index}"), index))
            .collect::<Vec<_>>();
        let layer_pes = pes
            .iter()
            .map(|pe| layer_pe(&pe.name, 1))
            .collect::<Vec<_>>();
        let layers = (0..1_000)
            .map(|index| {
                layer(
                    index,
                    layer_pes
                        .iter()
                        .map(|pe| layer_pe(&pe.name, pe.machine_ops.total))
                        .collect(),
                )
            })
            .collect();
        let model = AppModel::new(report(layers, pes));

        let relation = build(&model);

        assert_eq!(relation.matching_edges, 10_000);
        assert_eq!(relation.total, 10_000.0);
        assert_eq!(relation.edges.len(), 5_000);
        assert_eq!(relation.omitted_edges, 5_000);
        assert_eq!(relation.sources.len() + relation.omitted_sources, 1_000);
        assert!(relation.sources.len() <= 500);
        assert!(relation.targets.len() <= 5_000);
    }

    #[test]
    fn retains_the_strongest_edges() {
        let pes = (0..5_001)
            .map(|index| pe(&format!("pe {index}"), index))
            .collect::<Vec<_>>();
        let layer_pes = pes
            .iter()
            .enumerate()
            .map(|(index, pe)| layer_pe(&pe.name, (index + 1) as u64))
            .collect();
        let model = AppModel::new(report(vec![layer(0, layer_pes)], pes));

        let relation = build(&model);

        assert_eq!(relation.edges.len(), 5_000);
        assert_eq!(relation.edges.first().unwrap().value, 5_001.0);
        assert_eq!(relation.edges.last().unwrap().value, 2.0);
        assert!(relation.edges.iter().all(|edge| edge.target != "pe 0"));
    }

    #[test]
    fn retains_a_selected_source_outside_the_initial_window() {
        let layers = (0..501)
            .map(|index| layer(index, vec![layer_pe("pe", 1)]))
            .collect();
        let mut model = AppModel::new(report(layers, vec![pe("pe", 0)]));
        model.state.selected_layer = Some("layer 500".into());

        let relation = build(&model);

        assert!(relation.sources.iter().any(|node| node.id == "layer 500"));
        assert_eq!(relation.omitted_sources, 1);
    }

    #[test]
    fn retains_an_edge_for_the_selected_source() {
        let pes = (0..11)
            .map(|index| pe(&format!("pe {index}"), index))
            .collect::<Vec<_>>();
        let layers = (0..501)
            .map(|index| {
                let value = if index == 500 { 1 } else { 2 };
                layer(
                    index,
                    pes.iter().map(|pe| layer_pe(&pe.name, value)).collect(),
                )
            })
            .collect();
        let mut model = AppModel::new(report(layers, pes));
        model.state.selected_layer = Some("layer 500".into());

        let relation = build(&model);

        assert!(relation.sources.iter().any(|node| node.id == "layer 500"));
        assert!(relation.edges.iter().any(|edge| edge.source == "layer 500"));
    }

    #[test]
    fn aggregates_memory_relationships_from_tensor_transfers() {
        let mut data = report(vec![layer(0, vec![layer_pe("pe", 1)])], vec![pe("pe", 0)]);
        data.memory.platform_memories = vec![memory("m0", 0), memory("m1", 2)];
        data.tensors = vec![tensor_with_read("tensor", "pe", "layer 0")];
        let mut model = AppModel::new(data);
        model.state.relationship_measure = RelationshipMeasure::Read;

        for (mode, source) in [
            (RelationshipMode::LayerMemory, "layer 0"),
            (RelationshipMode::PeMemory, "pe"),
            (RelationshipMode::TensorMemory, "tensor"),
        ] {
            let edges = collect_memory_edges(&model, mode);

            assert_eq!(edges[source]["m0"], 2.0);
            assert_eq!(edges[source]["m1"], 2.0);
        }
    }

    fn report(layers: Vec<LayerSummary>, pes: Vec<PeSummary>) -> ReportData {
        ReportData {
            summary: Summary {
                timetable: "test".into(),
                platform: None,
                overlay: None,
                nodes: 0,
                compute_nodes: 0,
                total_machine_ops: 0,
                tensor_nodes: 0,
                total_tensor_read_bytes: 0,
                total_tensor_write_bytes: 0,
                data_edges: 0,
                active_pes: pes.len() as u64,
            },
            layers,
            ops: Vec::new(),
            machine_ops: Vec::new(),
            memory: MemorySummary {
                min_addr: None,
                max_addr: None,
                total_memory_read_bytes: 0,
                total_memory_write_bytes: 0,
                platform_memories: Vec::new(),
            },
            tensors: Vec::new(),
            overlay_metrics: BTreeMap::new(),
            pes,
            platform: None,
            warnings: Vec::new(),
        }
    }

    fn layer(index: usize, pes: Vec<LayerPeSummary>) -> LayerSummary {
        LayerSummary {
            name: format!("layer {index}"),
            compute_nodes: 0,
            machine_ops: MachineOpSummary::default(),
            tensor_count: 0,
            tensor_read_bytes: 0,
            tensor_write_bytes: 0,
            by_op: BTreeMap::new(),
            pes,
        }
    }

    fn layer_pe(name: &str, machine_ops: u64) -> LayerPeSummary {
        LayerPeSummary {
            name: name.into(),
            compute_nodes: 1,
            machine_ops: MachineOpSummary {
                total: machine_ops,
                adds: machine_ops,
                ..MachineOpSummary::default()
            },
            by_op: BTreeMap::new(),
            tensor_count: 0,
            tensor_read_bytes: 0,
            tensor_write_bytes: 0,
        }
    }

    fn pe(name: &str, col: usize) -> PeSummary {
        PeSummary {
            name: name.into(),
            row: 0,
            col: col as u64,
            total_nodes: 1,
            machine_ops: MachineOpSummary::default(),
            machine_ops_by_layer: BTreeMap::new(),
            tensor_read_bytes: 0,
            tensor_write_bytes: 0,
            by_layer: BTreeMap::new(),
            by_op: BTreeMap::new(),
            present_in_timetable: true,
            present_in_platform: false,
            platform_config: None,
            overlays: BTreeMap::new(),
        }
    }

    fn memory(name: &str, base_addr: u64) -> MemoryDeviceSummary {
        MemoryDeviceSummary {
            name: name.into(),
            kind: "sram".into(),
            base_addr,
            capacity_bytes: 2,
            allocated_bytes: 0,
            read_bytes: 0,
            write_bytes: 0,
            tensor_count: 0,
            tensors: Vec::new(),
        }
    }

    fn tensor_with_read(id: &str, pe: &str, layer: &str) -> TensorSummary {
        TensorSummary {
            id: id.into(),
            addr: 0,
            num_bytes: 4,
            dtype: "int8".into(),
            shape: vec![4],
            writes_by_pe: Vec::new(),
            reads_by_pe: vec![TensorPeTraffic {
                pe: pe.into(),
                bytes: 4,
                edge_count: 1,
                by_layer: BTreeMap::new(),
                transfers: vec![TensorTransfer {
                    layer: Some(layer.into()),
                    access: TensorAccess {
                        first_element: 0,
                        elements_per_range: 4,
                        strides: Vec::new(),
                        bits_per_element: 8,
                        num_access_bytes: 4,
                    },
                }],
            }],
        }
    }
}
