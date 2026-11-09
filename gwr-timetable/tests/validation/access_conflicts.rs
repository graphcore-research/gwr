// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::processing_element::program::syntax::{
    AssignmentSpec, BoundsMode, ExprSpec, IndexSpec, ProgramSpec,
};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_timetable::timetable_file::NodeSection;

use super::common::{compute_with_program, data_edge, tensor, timetable, view};

#[test]
fn rejects_overlapping_reads_and_writes() {
    let file = timetable(
        vec![
            tensor("input", 0x1000, DataType::Fp32, &[4]),
            compute_with_program(
                "compute",
                read_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            tensor("output", 0x1008, DataType::Fp32, &[4]),
        ],
        vec![
            data_edge("input", "compute"),
            data_edge("compute", "output"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("Node 'compute' reads tensor 'input'"));
    assert!(error.contains("writes tensor 'output' to overlapping range"));
}

#[test]
fn accepts_views_in_adjacent_bytes() {
    let file = timetable(
        vec![
            tensor("input", 0x1000, DataType::Int4, &[4]),
            compute_with_program(
                "compute",
                read_program(1, &[(0, &[1])]),
                vec![view(&[0], &[1])],
                vec![view(&[2], &[1])],
            ),
            tensor("output", 0x1000, DataType::Int4, &[4]),
        ],
        vec![
            data_edge("input", "compute"),
            data_edge("compute", "output"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn accepts_disjoint_strided_views() {
    let file = timetable(
        vec![
            tensor("input", 0x1000, DataType::Int8, &[2, 4]),
            compute_with_program(
                "compute",
                read_program(1, &[(0, &[2, 2])]),
                vec![view(&[0, 0], &[2, 2])],
                vec![view(&[0, 2], &[2, 2])],
            ),
            tensor("output", 0x1000, DataType::Int8, &[2, 4]),
        ],
        vec![
            data_edge("input", "compute"),
            data_edge("compute", "output"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn rejects_overlapping_strided_views() {
    let file = timetable(
        vec![
            tensor("input", 0x1000, DataType::Int8, &[2, 4]),
            compute_with_program(
                "compute",
                read_program(1, &[(0, &[2, 2])]),
                vec![view(&[0, 0], &[2, 2])],
                vec![view(&[0, 1], &[2, 2])],
            ),
            tensor("output", 0x1000, DataType::Int8, &[2, 4]),
        ],
        vec![
            data_edge("input", "compute"),
            data_edge("compute", "output"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("writes tensor 'output' to overlapping range"));
}

#[test]
fn finds_the_first_overlap_in_a_large_strided_view() {
    let file = timetable(
        vec![
            tensor("input", 0x1000, DataType::Int8, &[100_000_000, 2]),
            compute_with_program(
                "compute",
                read_program(1, &[(0, &[100_000_000, 1])]),
                vec![view(&[0, 0], &[100_000_000, 1])],
                vec![view(&[0, 0], &[100_000_000, 1])],
            ),
            tensor("output", 0x1000, DataType::Int8, &[100_000_000, 2]),
        ],
        vec![
            data_edge("input", "compute"),
            data_edge("compute", "output"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("writes tensor 'output' to overlapping range"));
}

#[test]
fn rejects_views_sharing_a_packed_byte() {
    let file = timetable(
        vec![
            tensor("input", 0x1000, DataType::Int4, &[2]),
            compute_with_program(
                "compute",
                read_program(1, &[(0, &[1])]),
                vec![view(&[1], &[1])],
                vec![view(&[0], &[1])],
            ),
            tensor("output", 0x1000, DataType::Int4, &[2]),
        ],
        vec![
            data_edge("input", "compute"),
            data_edge("compute", "output"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("writes tensor 'output' to overlapping range"));
}

#[test]
fn rejects_packed_views_with_multiple_odd_strides() {
    let file = timetable(
        vec![
            tensor("input", 0x1000, DataType::Int4, &[2, 3, 3]),
            compute_with_program(
                "compute",
                read_program(2, &[(1, &[1])]),
                vec![view(&[0, 0, 0], &[2, 2, 1]), None],
                vec![None],
            ),
            tensor("output", 0x1001, DataType::Int8, &[1]),
            source("source", DataType::Int8, &[1]),
        ],
        vec![
            data_edge("input", "compute"),
            data_edge("compute", "output"),
            data_edge("source", "compute.1"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("writes tensor 'output' to overlapping range"));
    assert!(error.contains("0x1000..0x1002"));
    assert!(error.contains("0x1001..0x1002"));
}

#[test]
fn rejects_overlapping_outputs_from_one_compute_node() {
    let file = timetable(
        vec![
            source("source0", DataType::Fp32, &[2]),
            source("source1", DataType::Fp32, &[2]),
            compute_with_program(
                "compute",
                read_program(2, &[(0, &[2]), (1, &[2])]),
                vec![None, None],
                vec![None, None],
            ),
            tensor("output0", 0x1000, DataType::Fp32, &[2]),
            tensor("output1", 0x1004, DataType::Fp32, &[2]),
        ],
        vec![
            data_edge("source0", "compute.0"),
            data_edge("source1", "compute.1"),
            data_edge("compute.0", "output0"),
            data_edge("compute.1", "output1"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("Node 'compute' writes tensor 'output0'"));
    assert!(error.contains("tensor 'output1' to overlapping range"));
}

#[test]
fn accepts_adjacent_outputs_from_one_compute_node() {
    let file = timetable(
        vec![
            source("source0", DataType::Fp32, &[1]),
            source("source1", DataType::Fp32, &[1]),
            compute_with_program(
                "compute",
                read_program(2, &[(0, &[1]), (1, &[1])]),
                vec![None, None],
                vec![None, None],
            ),
            tensor("output0", 0x1000, DataType::Fp32, &[1]),
            tensor("output1", 0x1004, DataType::Fp32, &[1]),
        ],
        vec![
            data_edge("source0", "compute.0"),
            data_edge("source1", "compute.1"),
            data_edge("compute.0", "output0"),
            data_edge("compute.1", "output1"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn rejects_unordered_writers_to_one_tensor() {
    let file = timetable(
        vec![
            source("source0", DataType::Int8, &[4]),
            source("source1", DataType::Int8, &[4]),
            compute_with_program(
                "producer0",
                read_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            compute_with_program(
                "producer1",
                read_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            tensor("result", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
            data_edge("source0", "producer0"),
            data_edge("source1", "producer1"),
            data_edge("producer0", "result.0"),
            data_edge("producer1", "result.1"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("Nodes 'producer0' and 'producer1'"));
    assert!(error.contains("write tensor 'result' to overlapping memory ranges"));
}

#[test]
fn accepts_disjoint_unordered_writers_to_one_tensor() {
    let file = timetable(
        vec![
            source("source0", DataType::Int8, &[2]),
            source("source1", DataType::Int8, &[2]),
            compute_with_program(
                "producer0",
                read_program(1, &[(0, &[2])]),
                vec![None],
                vec![view(&[0], &[2])],
            ),
            compute_with_program(
                "producer1",
                read_program(1, &[(0, &[2])]),
                vec![None],
                vec![view(&[2], &[2])],
            ),
            tensor("result", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
            data_edge("source0", "producer0"),
            data_edge("source1", "producer1"),
            data_edge("producer0", "result.0"),
            data_edge("producer1", "result.1"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn rejects_unordered_writers_sharing_a_packed_byte() {
    let file = timetable(
        vec![
            source("source0", DataType::Int4, &[1]),
            source("source1", DataType::Int4, &[1]),
            compute_with_program(
                "producer0",
                read_program(1, &[(0, &[1])]),
                vec![None],
                vec![view(&[0], &[1])],
            ),
            compute_with_program(
                "producer1",
                read_program(1, &[(0, &[1])]),
                vec![None],
                vec![view(&[1], &[1])],
            ),
            tensor("result", 0x1000, DataType::Int4, &[2]),
        ],
        vec![
            data_edge("source0", "producer0"),
            data_edge("source1", "producer1"),
            data_edge("producer0", "result.0"),
            data_edge("producer1", "result.1"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("write tensor 'result' to overlapping memory ranges"));
}

#[test]
fn accepts_dependency_ordered_writers_to_one_tensor() {
    let file = timetable(
        vec![
            source("source0", DataType::Int8, &[4]),
            source("source1", DataType::Int8, &[4]),
            compute_with_program(
                "producer0",
                read_program(2, &[(0, &[4]), (1, &[4])]),
                vec![None, None],
                vec![None, None],
            ),
            tensor("gate", 0x2000, DataType::Int8, &[4]),
            compute_with_program(
                "producer1",
                read_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            tensor("result", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
            data_edge("source0", "producer0.0"),
            data_edge("source1", "producer0.1"),
            data_edge("producer0.0", "gate"),
            data_edge("producer0.1", "result.0"),
            data_edge("gate", "producer1"),
            data_edge("producer1", "result.1"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn rejects_unordered_writers_to_aliased_tensors() {
    let file = timetable(
        vec![
            source("source0", DataType::Int8, &[4]),
            source("source1", DataType::Int8, &[4]),
            compute_with_program(
                "producer0",
                read_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            tensor("output0", 0x1000, DataType::Int8, &[4]),
            compute_with_program(
                "producer1",
                read_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            tensor("output1", 0x1002, DataType::Int8, &[4]),
        ],
        vec![
            data_edge("source0", "producer0"),
            data_edge("source1", "producer1"),
            data_edge("producer0", "output0"),
            data_edge("producer1", "output1"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("Nodes 'producer0' and 'producer1'"));
    assert!(error.contains("write tensors 'output0' and 'output1'"));
    assert!(error.contains("0x1000..0x1004"));
    assert!(error.contains("0x1002..0x1006"));
}

#[test]
fn accepts_disjoint_writers_to_aliased_tensors() {
    let file = timetable(
        vec![
            source("source0", DataType::Int8, &[2]),
            source("source1", DataType::Int8, &[2]),
            compute_with_program(
                "producer0",
                read_program(1, &[(0, &[2])]),
                vec![None],
                vec![view(&[0], &[2])],
            ),
            tensor("output0", 0x1000, DataType::Int8, &[4]),
            compute_with_program(
                "producer1",
                read_program(1, &[(0, &[2])]),
                vec![None],
                vec![view(&[2], &[2])],
            ),
            tensor("output1", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
            data_edge("source0", "producer0"),
            data_edge("source1", "producer1"),
            data_edge("producer0", "output0"),
            data_edge("producer1", "output1"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn accepts_wide_finite_stride_writers() {
    const COUNT: usize = 100_000_000;

    let file = timetable(
        vec![
            source("source0", DataType::Int8, &[COUNT, 2]),
            source("source1", DataType::Int8, &[COUNT, 2]),
            compute_with_program(
                "producer0",
                read_program(1, &[(0, &[COUNT, 2])]),
                vec![None],
                vec![view(&[0, 0], &[COUNT, 2])],
            ),
            tensor("output0", 0, DataType::Int8, &[COUNT, 1_000_000_001]),
            compute_with_program(
                "producer1",
                read_program(1, &[(0, &[COUNT, 2])]),
                vec![None],
                vec![view(&[0, 0], &[COUNT, 2])],
            ),
            tensor("output1", 2, DataType::Int8, &[COUNT, 1_000_000_005]),
        ],
        vec![
            data_edge("source0", "producer0"),
            data_edge("source1", "producer1"),
            data_edge("producer0", "output0"),
            data_edge("producer1", "output1"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn rejects_aliased_tensor_views_sharing_a_packed_byte() {
    let file = timetable(
        vec![
            source("source0", DataType::Int4, &[1]),
            source("source1", DataType::Int4, &[1]),
            compute_with_program(
                "producer0",
                read_program(1, &[(0, &[1])]),
                vec![None],
                vec![view(&[0], &[1])],
            ),
            tensor("output0", 0x1000, DataType::Int4, &[2]),
            compute_with_program(
                "producer1",
                read_program(1, &[(0, &[1])]),
                vec![None],
                vec![view(&[1], &[1])],
            ),
            tensor("output1", 0x1000, DataType::Int4, &[2]),
        ],
        vec![
            data_edge("source0", "producer0"),
            data_edge("source1", "producer1"),
            data_edge("producer0", "output0"),
            data_edge("producer1", "output1"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("write tensors 'output0' and 'output1'"));
}

#[test]
fn rejects_overlapping_strided_writes_to_aliased_tensors() {
    let file = timetable(
        vec![
            source("source0", DataType::Int8, &[2, 2]),
            source("source1", DataType::Int8, &[2, 2]),
            compute_with_program(
                "producer0",
                read_program(1, &[(0, &[2, 2])]),
                vec![None],
                vec![view(&[0, 0], &[2, 2])],
            ),
            tensor("output0", 0x1000, DataType::Int8, &[2, 4]),
            compute_with_program(
                "producer1",
                read_program(1, &[(0, &[2, 2])]),
                vec![None],
                vec![view(&[0, 1], &[2, 2])],
            ),
            tensor("output1", 0x1000, DataType::Int8, &[2, 4]),
        ],
        vec![
            data_edge("source0", "producer0"),
            data_edge("source1", "producer1"),
            data_edge("producer0", "output0"),
            data_edge("producer1", "output1"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("write tensors 'output0' and 'output1'"));
}

#[test]
fn accepts_dependency_ordered_writes_to_aliased_tensors() {
    let file = timetable(
        vec![
            source("source0", DataType::Int8, &[1]),
            source("source1", DataType::Int8, &[4]),
            source("source2", DataType::Int8, &[4]),
            compute_with_program(
                "producer0",
                read_program(2, &[(0, &[1]), (1, &[4])]),
                vec![None, None],
                vec![None, None],
            ),
            tensor("gate", 0x2000, DataType::Int8, &[1]),
            tensor("output0", 0x1000, DataType::Int8, &[4]),
            compute_with_program(
                "producer1",
                read_program(2, &[(1, &[4])]),
                vec![None, None],
                vec![None],
            ),
            tensor("output1", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
            data_edge("source0", "producer0.0"),
            data_edge("source1", "producer0.1"),
            data_edge("source2", "producer1.1"),
            data_edge("producer0.0", "gate"),
            data_edge("producer0.1", "output0"),
            data_edge("gate", "producer1"),
            data_edge("producer1", "output1"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn rejects_unordered_readers_and_writers_to_aliased_tensors() {
    let file = timetable(
        vec![
            tensor("input", 0x1000, DataType::Int8, &[4]),
            compute_with_program("reader", read_program(1, &[]), vec![None], vec![]),
            compute_with_program(
                "writer",
                read_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            tensor("output", 0x1002, DataType::Int8, &[4]),
            source("source", DataType::Int8, &[4]),
        ],
        vec![
            data_edge("input", "reader"),
            data_edge("writer", "output"),
            data_edge("source", "writer"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("Node 'reader' reads tensor 'input'"));
    assert!(error.contains("unordered node 'writer' writes tensor 'output'"));
    assert!(error.contains("0x1000..0x1004"));
    assert!(error.contains("0x1002..0x1006"));
}

#[test]
fn accepts_dependency_ordered_readers_and_writers_to_aliased_tensors() {
    let file = timetable(
        vec![
            source("source0", DataType::Int8, &[1]),
            source("source1", DataType::Int8, &[4]),
            compute_with_program(
                "writer",
                read_program(2, &[(0, &[1]), (1, &[4])]),
                vec![None, None],
                vec![None, None],
            ),
            tensor("gate", 0x2000, DataType::Int8, &[1]),
            tensor("output", 0x1000, DataType::Int8, &[4]),
            tensor("input", 0x1000, DataType::Int8, &[4]),
            compute_with_program("reader", read_program(2, &[]), vec![None, None], vec![]),
        ],
        vec![
            data_edge("source0", "writer.0"),
            data_edge("source1", "writer.1"),
            data_edge("writer.0", "gate"),
            data_edge("writer.1", "output"),
            data_edge("gate", "reader.0"),
            data_edge("input", "reader.1"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn indexes_many_disjoint_partitions_by_address() {
    const NUM_PARTITIONS: usize = 10_000;

    let mut nodes = Vec::with_capacity(NUM_PARTITIONS * 2 + 1);
    let mut edges = Vec::with_capacity(NUM_PARTITIONS * 2);
    for index in 0..NUM_PARTITIONS {
        let compute_id = format!("producer{index}");
        let source_id = format!("source{index}");
        nodes.push(compute_with_program(
            &compute_id,
            read_program(1, &[(0, &[1])]),
            vec![None],
            vec![view(&[index], &[1])],
        ));
        nodes.push(source(&source_id, DataType::Int8, &[1]));
        edges.push(data_edge(&compute_id, "output"));
        edges.push(data_edge(&source_id, &compute_id));
    }
    nodes.push(tensor("output", 0, DataType::Int8, &[NUM_PARTITIONS]));

    timetable(nodes, edges).validate().unwrap();
}

#[test]
fn validates_a_long_chain_of_ordered_alias_writes() {
    const NUM_WRITERS: usize = 1_000;

    let mut nodes = Vec::with_capacity(NUM_WRITERS * 5);
    let mut edges = Vec::with_capacity(NUM_WRITERS * 5);
    for index in 0..NUM_WRITERS {
        let compute_id = format!("producer{index}");
        let gate_id = format!("gate{index}");
        let output_id = format!("output{index}");

        let source0 = format!("source{index}_0");
        let source1 = format!("source{index}_1");
        nodes.push(source(&source0, DataType::Int8, &[1]));
        nodes.push(source(&source1, DataType::Int8, &[1]));

        let first_source_port = usize::from(index != 0);
        nodes.push(compute_with_program(
            &compute_id,
            read_program(
                first_source_port + 2,
                &[(first_source_port, &[1]), (first_source_port + 1, &[1])],
            ),
            vec![None; first_source_port + 2],
            vec![None, None],
        ));
        nodes.push(tensor(
            &gate_id,
            0x10_0000 + index as u64,
            DataType::Int8,
            &[1],
        ));
        nodes.push(tensor(&output_id, 0, DataType::Int8, &[1]));
        if index != 0 {
            edges.push(data_edge(&format!("gate{}", index - 1), &compute_id));
        }

        edges.push(data_edge(
            &source0,
            &format!("{compute_id}.{first_source_port}"),
        ));
        edges.push(data_edge(
            &source1,
            &format!("{compute_id}.{}", first_source_port + 1),
        ));
        edges.push(data_edge(&format!("{compute_id}.0"), &gate_id));
        edges.push(data_edge(&format!("{compute_id}.1"), &output_id));
    }

    timetable(nodes, edges).validate().unwrap();
}

#[test]
fn validates_many_interleaved_dependency_chains() {
    const NUM_CHAINS: usize = 64;
    const NUM_STEPS: usize = 64;

    let mut nodes = Vec::with_capacity(NUM_CHAINS * (3 * NUM_STEPS - 2));
    let mut edges = Vec::with_capacity(NUM_CHAINS * (3 * NUM_STEPS - 2));
    for step in 0..NUM_STEPS {
        for chain in 0..NUM_CHAINS {
            let compute_id = format!("compute{chain}_{step}");
            let has_output = step + 1 != NUM_STEPS;
            let input_count = usize::from(step != 0) + usize::from(has_output);
            let source_port = usize::from(step != 0);
            let outputs: Vec<(usize, &[usize])> = if has_output {
                vec![(source_port, &[1])]
            } else {
                vec![]
            };
            nodes.push(compute_with_program(
                &compute_id,
                read_program(input_count, &outputs),
                vec![None; input_count],
                vec![None; usize::from(has_output)],
            ));
            if has_output {
                let source_id = format!("source{chain}_{step}");
                nodes.push(source(&source_id, DataType::Int8, &[1]));
                edges.push(data_edge(
                    &source_id,
                    &format!("{compute_id}.{source_port}"),
                ));
            }
        }
    }
    for step in 0..NUM_STEPS - 1 {
        for chain in 0..NUM_CHAINS {
            let gate = format!("gate{chain}_{step}");
            let address = (step * NUM_CHAINS + chain) as u64;
            nodes.push(tensor(&gate, address, DataType::Int8, &[1]));
            edges.push(data_edge(&format!("compute{chain}_{step}"), &gate));
            edges.push(data_edge(&gate, &format!("compute{chain}_{}", step + 1)));
        }
    }

    timetable(nodes, edges).validate().unwrap();
}

fn source(id: &str, dtype: DataType, shape: &[usize]) -> NodeSection {
    tensor(id, SOURCE_ADDRESS, dtype, shape)
}

// Source tensors are explicit in each fixture. They share an address because
// they are only read, and sit beyond the output address ranges being tested.
const SOURCE_ADDRESS: u64 = 0xf000_0000_0000_0000;

fn read_program(input_count: usize, outputs: &[(usize, &[usize])]) -> ProgramSpec {
    let inputs = (0..input_count)
        .map(|index| format!("input{index}"))
        .collect::<Vec<_>>();
    let output_names = (0..outputs.len())
        .map(|index| format!("output{index}"))
        .collect::<Vec<_>>();
    let assignments = outputs
        .iter()
        .enumerate()
        .map(|(output_index, (input_index, shape))| {
            let axes = (0..shape.len())
                .map(|dimension| format!("d{output_index}_{dimension}"))
                .collect::<Vec<_>>();
            AssignmentSpec {
                output: Some(output_names[output_index].clone()),
                indices_output: None,
                axes: axes.clone(),
                expr: ExprSpec::Read {
                    tensor: inputs[*input_index].clone(),
                    indices: axes.into_iter().map(IndexSpec::Axis).collect(),
                    bounds: BoundsMode::RequireInRange,
                },
            }
        })
        .collect();
    ProgramSpec {
        metadata_inputs: vec![],
        indexed_metadata_inputs: vec![],
        local_round_policy: None,
        inputs,
        outputs: output_names,
        temporaries: BTreeMap::default(),
        assignments,
        extents: BTreeMap::default(),
    }
}
