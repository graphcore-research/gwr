// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_engine::types::SimError;
use gwr_models::processing_element::operators::HasShape;
use gwr_timetable::{ComputeTensorDirection, TimetableGraph};

use super::graph::layer_name;
use super::{PeTable, add_u64, pe_index_for_node, u64_from_usize};
use crate::model::{
    TensorAccess, TensorLayerTraffic, TensorPeTraffic, TensorSummary, TensorTransfer,
};

pub(super) fn build_tensor_summaries(
    graph: &TimetableGraph,
    node_layers: &[usize],
    node_pe_indices: &[Option<usize>],
    pes: &mut PeTable,
) -> Result<BTreeMap<usize, TensorSummary>, SimError> {
    let mut tensors = graph
        .nodes()
        .iter()
        .enumerate()
        .filter_map(|(index, node)| node.tensor().map(|tensor| (index, node, tensor)))
        .map(|(index, node, tensor)| {
            Ok((
                index,
                TensorSummary {
                    id: node.id().to_string(),
                    addr: tensor.addr(),
                    num_bytes: u64_from_usize(tensor.num_bytes(), "tensor byte count")?,
                    dtype: format!("{:?}", tensor.dtype()).to_lowercase(),
                    shape: tensor
                        .shape()
                        .dims()
                        .iter()
                        .map(|extent| u64_from_usize(*extent, "tensor dimension"))
                        .collect::<Result<_, _>>()?,
                    writes_by_pe: Vec::new(),
                    reads_by_pe: Vec::new(),
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, SimError>>()?;

    for connection in graph
        .edges()
        .iter()
        .filter_map(|edge| edge.tensor_connection())
    {
        let compute_node = &graph.nodes()[connection.compute_node()];
        let pe_index = pe_index_for_node(graph, node_pe_indices, connection.compute_node())?;
        let pe_name = pes.get(pe_index).name.clone();
        let transfer = TensorTransfer {
            layer: Some(layer_name(node_layers[connection.compute_node()])),
            access: TensorAccess::try_from(connection.view().layout()).map_err(|error| {
                SimError(format!(
                    "Tensor connection between '{}' and '{}': {error}",
                    graph.nodes()[connection.tensor_node()].id(),
                    compute_node.id(),
                ))
            })?,
        };
        let tensor = tensors.get_mut(&connection.tensor_node()).ok_or_else(|| {
            SimError(format!(
                "Tensor connection identifies non-tensor node '{}'",
                graph.nodes()[connection.tensor_node()].id()
            ))
        })?;
        let traffic = if connection.direction() == ComputeTensorDirection::Input {
            add_u64(
                &mut pes.get_mut(pe_index).tensor_read_bytes,
                transfer.access.num_access_bytes,
                "PE tensor read byte total",
            )?;
            &mut tensor.reads_by_pe
        } else {
            add_u64(
                &mut pes.get_mut(pe_index).tensor_write_bytes,
                transfer.access.num_access_bytes,
                "PE tensor write byte total",
            )?;
            &mut tensor.writes_by_pe
        };
        push_transfer(traffic, &pe_name, transfer)?;
    }

    Ok(tensors)
}

pub(super) fn tensor_traffic_totals(
    tensors: &BTreeMap<usize, TensorSummary>,
) -> Result<(u64, u64), SimError> {
    let mut read_bytes = 0;
    let mut write_bytes = 0;
    for tensor in tensors.values() {
        for traffic in &tensor.reads_by_pe {
            add_u64(
                &mut read_bytes,
                traffic.bytes,
                "Report tensor read byte total",
            )?;
        }
        for traffic in &tensor.writes_by_pe {
            add_u64(
                &mut write_bytes,
                traffic.bytes,
                "Report tensor write byte total",
            )?;
        }
    }
    Ok((read_bytes, write_bytes))
}

fn push_transfer(
    traffic_by_pe: &mut Vec<TensorPeTraffic>,
    pe: &str,
    transfer: TensorTransfer,
) -> Result<(), SimError> {
    let bytes = transfer.access.num_access_bytes;
    let layer = transfer.layer.as_deref();
    if let Some(traffic) = traffic_by_pe.iter_mut().find(|traffic| traffic.pe == pe) {
        add_u64(&mut traffic.bytes, bytes, "Tensor traffic byte total")?;
        add_u64(&mut traffic.edge_count, 1, "Tensor traffic edge count")?;
        if let Some(layer) = layer {
            add_layer_traffic(&mut traffic.by_layer, layer, bytes)?;
        }
        traffic.transfers.push(transfer);
    } else {
        let mut by_layer = BTreeMap::new();
        if let Some(layer) = layer {
            add_layer_traffic(&mut by_layer, layer, bytes)?;
        }
        traffic_by_pe.push(TensorPeTraffic {
            pe: pe.to_string(),
            bytes,
            edge_count: 1,
            by_layer,
            transfers: vec![transfer],
        });
    }
    traffic_by_pe.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| left.pe.cmp(&right.pe))
    });
    Ok(())
}

fn add_layer_traffic(
    by_layer: &mut BTreeMap<String, TensorLayerTraffic>,
    layer: &str,
    bytes: u64,
) -> Result<(), SimError> {
    let traffic = by_layer.entry(layer.to_string()).or_default();
    add_u64(&mut traffic.bytes, bytes, "Layer tensor traffic byte total")?;
    add_u64(
        &mut traffic.edge_count,
        1,
        "Layer tensor traffic edge count",
    )
}
