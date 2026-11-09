// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::processing_element::program::syntax::{
    AssignmentSpec, BoundsMode, ExprSpec, IndexSpec, ProgramSpec,
};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_timetable::timetable_file::{NodeSection, TimetableFile};

use super::common::{compute_with_program, data_edge, tensor, timetable, view};

fn disjoint_partitions(partition_count: usize) -> TimetableFile {
    let mut nodes = Vec::with_capacity(partition_count * 2 + 1);
    let mut edges = Vec::with_capacity(partition_count * 2);
    for index in 0..partition_count {
        let compute_id = format!("producer{index}");
        let source_id = format!("source{index}");
        nodes.push(compute_with_program(
            &compute_id,
            copy_assignments_program(1, &[(0, &[1])]),
            vec![None],
            vec![view(&[index], &[1])],
        ));
        nodes.push(read_only_source_tensor(&source_id, DataType::Int8, &[1]));
        edges.push(data_edge(&format!("{compute_id}.output0"), "output"));
        edges.push(data_edge(&source_id, &format!("{compute_id}.input0")));
    }
    nodes.push(tensor("output", 0, DataType::Int8, &[partition_count]));

    timetable(nodes, edges)
}

// Source tensors are explicit in each fixture. They share an address because
// they are only read, and sit beyond the output address ranges being tested.
const SOURCE_ADDRESS: u64 = 0xf000_0000_0000_0000;

fn ordered_alias_writes(writer_count: usize) -> TimetableFile {
    let mut nodes = Vec::with_capacity(writer_count * 5);
    let mut edges = Vec::with_capacity(writer_count * 5);
    for index in 0..writer_count {
        let compute_id = format!("producer{index}");
        let gate_id = format!("gate{index}");
        let output_id = format!("output{index}");

        let source0 = format!("source{index}_0");
        let source1 = format!("source{index}_1");
        nodes.push(read_only_source_tensor(&source0, DataType::Int8, &[1]));
        nodes.push(read_only_source_tensor(&source1, DataType::Int8, &[1]));

        let first_source_port = usize::from(index != 0);
        nodes.push(compute_with_program(
            &compute_id,
            copy_assignments_program(
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
            edges.push(data_edge(
                &format!("gate{}", index - 1),
                &format!("{compute_id}.input0"),
            ));
        }

        edges.push(data_edge(
            &source0,
            &format!("{compute_id}.input{first_source_port}"),
        ));
        edges.push(data_edge(
            &source1,
            &format!("{compute_id}.input{}", first_source_port + 1),
        ));
        edges.push(data_edge(&format!("{compute_id}.output0"), &gate_id));
        edges.push(data_edge(&format!("{compute_id}.output1"), &output_id));
    }

    timetable(nodes, edges)
}

fn interleaved_dependency_chains(chain_count: usize, step_count: usize) -> TimetableFile {
    let mut nodes = Vec::with_capacity(chain_count * (3 * step_count - 2));
    let mut edges = Vec::with_capacity(chain_count * (3 * step_count - 2));
    for step in 0..step_count {
        for chain in 0..chain_count {
            let compute_id = format!("compute{chain}_{step}");
            let has_output = step + 1 != step_count;
            let input_count = usize::from(step != 0) + usize::from(has_output);
            let source_port = usize::from(step != 0);
            let outputs: Vec<(usize, &[usize])> = if has_output {
                vec![(source_port, &[1])]
            } else {
                vec![]
            };
            nodes.push(compute_with_program(
                &compute_id,
                copy_assignments_program(input_count, &outputs),
                vec![None; input_count],
                vec![None; usize::from(has_output)],
            ));
            if has_output {
                let source_id = format!("source{chain}_{step}");
                nodes.push(read_only_source_tensor(&source_id, DataType::Int8, &[1]));
                edges.push(data_edge(
                    &source_id,
                    &format!("{compute_id}.input{source_port}"),
                ));
            }
        }
    }
    for step in 0..step_count - 1 {
        for chain in 0..chain_count {
            let gate = format!("gate{chain}_{step}");
            let address = (step * chain_count + chain) as u64;
            nodes.push(tensor(&gate, address, DataType::Int8, &[1]));
            edges.push(data_edge(&format!("compute{chain}_{step}.output0"), &gate));
            edges.push(data_edge(
                &gate,
                &format!("compute{chain}_{}.input0", step + 1),
            ));
        }
    }

    timetable(nodes, edges)
}

fn read_only_source_tensor(id: &str, dtype: DataType, shape: &[usize]) -> NodeSection {
    tensor(id, SOURCE_ADDRESS, dtype, shape)
}

fn copy_assignments_program(input_count: usize, outputs: &[(usize, &[usize])]) -> ProgramSpec {
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
        inputs,
        outputs: output_names,
        temporaries: BTreeMap::default(),
        assignments,
        extents: BTreeMap::default(),
    }
}

#[test]
fn rejects_overlapping_reads_and_writes() {
    let file = timetable(
        vec![
            tensor("input", 0x1000, DataType::Fp32, &[4]),
            compute_with_program(
                "compute",
                copy_assignments_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            tensor("output", 0x1008, DataType::Fp32, &[4]),
        ],
        vec![
            data_edge("input", "compute.input0"),
            data_edge("compute.output0", "output"),
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
                copy_assignments_program(1, &[(0, &[1])]),
                vec![view(&[0], &[1])],
                vec![view(&[2], &[1])],
            ),
            tensor("output", 0x1000, DataType::Int4, &[4]),
        ],
        vec![
            data_edge("input", "compute.input0"),
            data_edge("compute.output0", "output"),
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
                copy_assignments_program(1, &[(0, &[2, 2])]),
                vec![view(&[0, 0], &[2, 2])],
                vec![view(&[0, 2], &[2, 2])],
            ),
            tensor("output", 0x1000, DataType::Int8, &[2, 4]),
        ],
        vec![
            data_edge("input", "compute.input0"),
            data_edge("compute.output0", "output"),
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
                copy_assignments_program(1, &[(0, &[2, 2])]),
                vec![view(&[0, 0], &[2, 2])],
                vec![view(&[0, 1], &[2, 2])],
            ),
            tensor("output", 0x1000, DataType::Int8, &[2, 4]),
        ],
        vec![
            data_edge("input", "compute.input0"),
            data_edge("compute.output0", "output"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("writes tensor 'output' to overlapping range"));
}

#[test]
fn rejects_overlapping_large_strided_views() {
    let file = timetable(
        vec![
            tensor("input", 0x1000, DataType::Int8, &[100_000_000, 2]),
            compute_with_program(
                "compute",
                copy_assignments_program(1, &[(0, &[100_000_000, 1])]),
                vec![view(&[0, 0], &[100_000_000, 1])],
                vec![view(&[0, 0], &[100_000_000, 1])],
            ),
            tensor("output", 0x1000, DataType::Int8, &[100_000_000, 2]),
        ],
        vec![
            data_edge("input", "compute.input0"),
            data_edge("compute.output0", "output"),
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
                copy_assignments_program(1, &[(0, &[1])]),
                vec![view(&[1], &[1])],
                vec![view(&[0], &[1])],
            ),
            tensor("output", 0x1000, DataType::Int4, &[2]),
        ],
        vec![
            data_edge("input", "compute.input0"),
            data_edge("compute.output0", "output"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("writes tensor 'output' to overlapping range"));
}

#[test]
fn rejects_overlapping_packed_views_with_multiple_odd_strides() {
    let file = timetable(
        vec![
            tensor("input", 0x1000, DataType::Int4, &[2, 3, 3]),
            compute_with_program(
                "compute",
                copy_assignments_program(2, &[(1, &[1])]),
                vec![view(&[0, 0, 0], &[2, 2, 1]), None],
                vec![None],
            ),
            tensor("output", 0x1001, DataType::Int8, &[1]),
            read_only_source_tensor("source", DataType::Int8, &[1]),
        ],
        vec![
            data_edge("input", "compute.input0"),
            data_edge("compute.output0", "output"),
            data_edge("source", "compute.input1"),
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
            read_only_source_tensor("source0", DataType::Fp32, &[2]),
            read_only_source_tensor("source1", DataType::Fp32, &[2]),
            compute_with_program(
                "compute",
                copy_assignments_program(2, &[(0, &[2]), (1, &[2])]),
                vec![None, None],
                vec![None, None],
            ),
            tensor("output0", 0x1000, DataType::Fp32, &[2]),
            tensor("output1", 0x1004, DataType::Fp32, &[2]),
        ],
        vec![
            data_edge("source0", "compute.input0"),
            data_edge("source1", "compute.input1"),
            data_edge("compute.output0", "output0"),
            data_edge("compute.output1", "output1"),
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
            read_only_source_tensor("source0", DataType::Fp32, &[1]),
            read_only_source_tensor("source1", DataType::Fp32, &[1]),
            compute_with_program(
                "compute",
                copy_assignments_program(2, &[(0, &[1]), (1, &[1])]),
                vec![None, None],
                vec![None, None],
            ),
            tensor("output0", 0x1000, DataType::Fp32, &[1]),
            tensor("output1", 0x1004, DataType::Fp32, &[1]),
        ],
        vec![
            data_edge("source0", "compute.input0"),
            data_edge("source1", "compute.input1"),
            data_edge("compute.output0", "output0"),
            data_edge("compute.output1", "output1"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn rejects_unordered_writers_to_one_tensor() {
    let file = timetable(
        vec![
            read_only_source_tensor("source0", DataType::Int8, &[4]),
            read_only_source_tensor("source1", DataType::Int8, &[4]),
            compute_with_program(
                "producer0",
                copy_assignments_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            compute_with_program(
                "producer1",
                copy_assignments_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            tensor("result", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
            data_edge("source0", "producer0.input0"),
            data_edge("source1", "producer1.input0"),
            data_edge("producer0.output0", "result"),
            data_edge("producer1.output0", "result"),
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
            read_only_source_tensor("source0", DataType::Int8, &[2]),
            read_only_source_tensor("source1", DataType::Int8, &[2]),
            compute_with_program(
                "producer0",
                copy_assignments_program(1, &[(0, &[2])]),
                vec![None],
                vec![view(&[0], &[2])],
            ),
            compute_with_program(
                "producer1",
                copy_assignments_program(1, &[(0, &[2])]),
                vec![None],
                vec![view(&[2], &[2])],
            ),
            tensor("result", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
            data_edge("source0", "producer0.input0"),
            data_edge("source1", "producer1.input0"),
            data_edge("producer0.output0", "result"),
            data_edge("producer1.output0", "result"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn rejects_unordered_writers_sharing_a_packed_byte() {
    let file = timetable(
        vec![
            read_only_source_tensor("source0", DataType::Int4, &[1]),
            read_only_source_tensor("source1", DataType::Int4, &[1]),
            compute_with_program(
                "producer0",
                copy_assignments_program(1, &[(0, &[1])]),
                vec![None],
                vec![view(&[0], &[1])],
            ),
            compute_with_program(
                "producer1",
                copy_assignments_program(1, &[(0, &[1])]),
                vec![None],
                vec![view(&[1], &[1])],
            ),
            tensor("result", 0x1000, DataType::Int4, &[2]),
        ],
        vec![
            data_edge("source0", "producer0.input0"),
            data_edge("source1", "producer1.input0"),
            data_edge("producer0.output0", "result"),
            data_edge("producer1.output0", "result"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("write tensor 'result' to overlapping memory ranges"));
}

#[test]
fn accepts_dependency_ordered_writers_to_one_tensor() {
    let file = timetable(
        vec![
            read_only_source_tensor("source0", DataType::Int8, &[4]),
            read_only_source_tensor("source1", DataType::Int8, &[4]),
            compute_with_program(
                "producer0",
                copy_assignments_program(2, &[(0, &[4]), (1, &[4])]),
                vec![None, None],
                vec![None, None],
            ),
            tensor("gate", 0x2000, DataType::Int8, &[4]),
            compute_with_program(
                "producer1",
                copy_assignments_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            tensor("result", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
            data_edge("source0", "producer0.input0"),
            data_edge("source1", "producer0.input1"),
            data_edge("producer0.output0", "gate"),
            data_edge("producer0.output1", "result"),
            data_edge("gate", "producer1.input0"),
            data_edge("producer1.output0", "result"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn rejects_unordered_writers_to_aliased_tensors() {
    let file = timetable(
        vec![
            read_only_source_tensor("source0", DataType::Int8, &[4]),
            read_only_source_tensor("source1", DataType::Int8, &[4]),
            compute_with_program(
                "producer0",
                copy_assignments_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            tensor("output0", 0x1000, DataType::Int8, &[4]),
            compute_with_program(
                "producer1",
                copy_assignments_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            tensor("output1", 0x1002, DataType::Int8, &[4]),
        ],
        vec![
            data_edge("source0", "producer0.input0"),
            data_edge("source1", "producer1.input0"),
            data_edge("producer0.output0", "output0"),
            data_edge("producer1.output0", "output1"),
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
            read_only_source_tensor("source0", DataType::Int8, &[2]),
            read_only_source_tensor("source1", DataType::Int8, &[2]),
            compute_with_program(
                "producer0",
                copy_assignments_program(1, &[(0, &[2])]),
                vec![None],
                vec![view(&[0], &[2])],
            ),
            tensor("output0", 0x1000, DataType::Int8, &[4]),
            compute_with_program(
                "producer1",
                copy_assignments_program(1, &[(0, &[2])]),
                vec![None],
                vec![view(&[2], &[2])],
            ),
            tensor("output1", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
            data_edge("source0", "producer0.input0"),
            data_edge("source1", "producer1.input0"),
            data_edge("producer0.output0", "output0"),
            data_edge("producer1.output0", "output1"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn accepts_wide_finite_stride_writers() {
    const COUNT: usize = 100_000_000;

    let file = timetable(
        vec![
            read_only_source_tensor("source0", DataType::Int8, &[COUNT, 2]),
            read_only_source_tensor("source1", DataType::Int8, &[COUNT, 2]),
            compute_with_program(
                "producer0",
                copy_assignments_program(1, &[(0, &[COUNT, 2])]),
                vec![None],
                vec![view(&[0, 0], &[COUNT, 2])],
            ),
            tensor("output0", 0, DataType::Int8, &[COUNT, 1_000_000_001]),
            compute_with_program(
                "producer1",
                copy_assignments_program(1, &[(0, &[COUNT, 2])]),
                vec![None],
                vec![view(&[0, 0], &[COUNT, 2])],
            ),
            tensor("output1", 2, DataType::Int8, &[COUNT, 1_000_000_005]),
        ],
        vec![
            data_edge("source0", "producer0.input0"),
            data_edge("source1", "producer1.input0"),
            data_edge("producer0.output0", "output0"),
            data_edge("producer1.output0", "output1"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn rejects_aliased_tensor_views_sharing_a_packed_byte() {
    let file = timetable(
        vec![
            read_only_source_tensor("source0", DataType::Int4, &[1]),
            read_only_source_tensor("source1", DataType::Int4, &[1]),
            compute_with_program(
                "producer0",
                copy_assignments_program(1, &[(0, &[1])]),
                vec![None],
                vec![view(&[0], &[1])],
            ),
            tensor("output0", 0x1000, DataType::Int4, &[2]),
            compute_with_program(
                "producer1",
                copy_assignments_program(1, &[(0, &[1])]),
                vec![None],
                vec![view(&[1], &[1])],
            ),
            tensor("output1", 0x1000, DataType::Int4, &[2]),
        ],
        vec![
            data_edge("source0", "producer0.input0"),
            data_edge("source1", "producer1.input0"),
            data_edge("producer0.output0", "output0"),
            data_edge("producer1.output0", "output1"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("write tensors 'output0' and 'output1'"));
}

#[test]
fn rejects_overlapping_strided_writes_to_aliased_tensors() {
    let file = timetable(
        vec![
            read_only_source_tensor("source0", DataType::Int8, &[2, 2]),
            read_only_source_tensor("source1", DataType::Int8, &[2, 2]),
            compute_with_program(
                "producer0",
                copy_assignments_program(1, &[(0, &[2, 2])]),
                vec![None],
                vec![view(&[0, 0], &[2, 2])],
            ),
            tensor("output0", 0x1000, DataType::Int8, &[2, 4]),
            compute_with_program(
                "producer1",
                copy_assignments_program(1, &[(0, &[2, 2])]),
                vec![None],
                vec![view(&[0, 1], &[2, 2])],
            ),
            tensor("output1", 0x1000, DataType::Int8, &[2, 4]),
        ],
        vec![
            data_edge("source0", "producer0.input0"),
            data_edge("source1", "producer1.input0"),
            data_edge("producer0.output0", "output0"),
            data_edge("producer1.output0", "output1"),
        ],
    );

    let error = file.validate().unwrap_err().to_string();
    assert!(error.contains("write tensors 'output0' and 'output1'"));
}

#[test]
fn accepts_dependency_ordered_writes_to_aliased_tensors() {
    let file = timetable(
        vec![
            read_only_source_tensor("source0", DataType::Int8, &[1]),
            read_only_source_tensor("source1", DataType::Int8, &[4]),
            read_only_source_tensor("source2", DataType::Int8, &[4]),
            compute_with_program(
                "producer0",
                copy_assignments_program(2, &[(0, &[1]), (1, &[4])]),
                vec![None, None],
                vec![None, None],
            ),
            tensor("gate", 0x2000, DataType::Int8, &[1]),
            tensor("output0", 0x1000, DataType::Int8, &[4]),
            compute_with_program(
                "producer1",
                copy_assignments_program(2, &[(1, &[4])]),
                vec![None, None],
                vec![None],
            ),
            tensor("output1", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
            data_edge("source0", "producer0.input0"),
            data_edge("source1", "producer0.input1"),
            data_edge("source2", "producer1.input1"),
            data_edge("producer0.output0", "gate"),
            data_edge("producer0.output1", "output0"),
            data_edge("gate", "producer1.input0"),
            data_edge("producer1.output0", "output1"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn rejects_unordered_readers_and_writers_to_aliased_tensors() {
    let file = timetable(
        vec![
            tensor("input", 0x1000, DataType::Int8, &[4]),
            compute_with_program(
                "reader",
                copy_assignments_program(1, &[]),
                vec![None],
                vec![],
            ),
            compute_with_program(
                "writer",
                copy_assignments_program(1, &[(0, &[4])]),
                vec![None],
                vec![None],
            ),
            tensor("output", 0x1002, DataType::Int8, &[4]),
            read_only_source_tensor("source", DataType::Int8, &[4]),
        ],
        vec![
            data_edge("input", "reader.input0"),
            data_edge("writer.output0", "output"),
            data_edge("source", "writer.input0"),
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
            read_only_source_tensor("source0", DataType::Int8, &[1]),
            read_only_source_tensor("source1", DataType::Int8, &[4]),
            compute_with_program(
                "writer",
                copy_assignments_program(2, &[(0, &[1]), (1, &[4])]),
                vec![None, None],
                vec![None, None],
            ),
            tensor("gate", 0x2000, DataType::Int8, &[1]),
            tensor("output", 0x1000, DataType::Int8, &[4]),
            tensor("input", 0x1000, DataType::Int8, &[4]),
            compute_with_program(
                "reader",
                copy_assignments_program(2, &[]),
                vec![None, None],
                vec![],
            ),
        ],
        vec![
            data_edge("source0", "writer.input0"),
            data_edge("source1", "writer.input1"),
            data_edge("writer.output0", "gate"),
            data_edge("writer.output1", "output"),
            data_edge("gate", "reader.input0"),
            data_edge("input", "reader.input1"),
        ],
    );

    file.validate().unwrap();
}

#[test]
fn accepts_many_disjoint_output_partitions() {
    let file = disjoint_partitions(10_000);

    file.validate().unwrap();
}

#[test]
fn validates_a_long_chain_of_ordered_alias_writes() {
    let file = ordered_alias_writes(1_000);

    file.validate().unwrap();
}

#[test]
fn validates_many_interleaved_dependency_chains() {
    let file = interleaved_dependency_chains(64, 64);

    file.validate().unwrap();
}
