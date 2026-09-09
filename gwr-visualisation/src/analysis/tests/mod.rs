// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

mod graph;
mod memory;
mod overlays;
mod summaries;
mod tensor_traffic;

use std::path::Path;

use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::task::ComputeOp;
use gwr_timetable::TimetableGraph;
use gwr_timetable::timetable_file::{
    EdgeKind, EdgeSection, NodeSection, TensorConfigSection, TensorViewSection, TimetableFile,
};

use super::build_report;
use crate::model::ReportData;

fn graph(nodes: Vec<NodeSection>, edges: Vec<EdgeSection>) -> TimetableGraph {
    TimetableFile { nodes, edges }.into_graph().unwrap()
}

fn small_graph() -> TimetableGraph {
    TimetableFile::from_file(Path::new("../gwr-timetable/examples/small.yaml"))
        .unwrap()
        .into_graph()
        .unwrap()
}

fn tensor(id: &str, addr: u64, dtype: DataType, shape: &[usize]) -> NodeSection {
    NodeSection::Tensor {
        id: id.to_string(),
        config: TensorConfigSection {
            addr,
            dtype,
            shape: shape.to_vec(),
        },
    }
}

fn compute(
    id: &str,
    operation: ComputeOp,
    pe: Option<&str>,
    input_count: usize,
    output_count: usize,
) -> NodeSection {
    NodeSection::Compute {
        id: id.to_string(),
        op: operation,
        pe: pe.map(str::to_string),
        input_views: vec![None; input_count],
        output_views: vec![None; output_count],
    }
}

fn compute_with_views(
    id: &str,
    operation: ComputeOp,
    pe: Option<&str>,
    input_views: Vec<Option<TensorViewSection>>,
    output_views: Vec<Option<TensorViewSection>>,
) -> NodeSection {
    NodeSection::Compute {
        id: id.to_string(),
        op: operation,
        pe: pe.map(str::to_string),
        input_views,
        output_views,
    }
}

fn view(shape: &[usize], offsets: &[usize]) -> Option<TensorViewSection> {
    Some(TensorViewSection {
        offsets: offsets.to_vec(),
        shape: shape.to_vec(),
    })
}

fn data(from: &str, to: &str) -> EdgeSection {
    EdgeSection {
        from: from.to_string(),
        to: to.to_string(),
        kind: EdgeKind::Data,
    }
}

fn control(from: &str, to: &str) -> EdgeSection {
    EdgeSection {
        from: from.to_string(),
        to: to.to_string(),
        kind: EdgeKind::Control,
    }
}

fn report(graph: &TimetableGraph) -> ReportData {
    build_report(graph, Path::new("timetable.yaml"), None, None).unwrap()
}
