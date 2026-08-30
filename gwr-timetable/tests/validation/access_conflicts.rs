// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use super::common::*;

#[test]
fn rejects_overlapping_reads_and_writes() {
    let file = timetable(
        vec![
            tensor("input", 0x1000, DataType::Fp32, &[4]),
            compute("compute", vec![None], vec![None]),
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
            compute("compute", vec![view(&[0], &[1])], vec![view(&[2], &[1])]),
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
            compute(
                "compute",
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
            compute(
                "compute",
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
            compute(
                "compute",
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
            compute("compute", vec![view(&[1], &[1])], vec![view(&[0], &[1])]),
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
            compute("compute", vec![view(&[0, 0, 0], &[2, 2, 1])], vec![None]),
            tensor("output", 0x1001, DataType::Int8, &[1]),
        ],
        vec![
            data_edge("input", "compute"),
            data_edge("compute", "output"),
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
            compute("compute", vec![], vec![None, None]),
            tensor("output0", 0x1000, DataType::Fp32, &[2]),
            tensor("output1", 0x1004, DataType::Fp32, &[2]),
        ],
        vec![
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
            compute("compute", vec![], vec![None, None]),
            tensor("output0", 0x1000, DataType::Fp32, &[1]),
            tensor("output1", 0x1004, DataType::Fp32, &[1]),
        ],
        vec![
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
            compute("producer0", vec![], vec![None]),
            compute("producer1", vec![], vec![None]),
            tensor("result", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
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
            compute("producer0", vec![], vec![view(&[0], &[2])]),
            compute("producer1", vec![], vec![view(&[2], &[2])]),
            tensor("result", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
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
            compute("producer0", vec![], vec![view(&[0], &[1])]),
            compute("producer1", vec![], vec![view(&[1], &[1])]),
            tensor("result", 0x1000, DataType::Int4, &[2]),
        ],
        vec![
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
            compute("producer0", vec![], vec![None, None]),
            tensor("gate", 0x2000, DataType::Int8, &[4]),
            compute("producer1", vec![None], vec![None]),
            tensor("result", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
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
            compute("producer0", vec![], vec![None]),
            tensor("output0", 0x1000, DataType::Int8, &[4]),
            compute("producer1", vec![], vec![None]),
            tensor("output1", 0x1002, DataType::Int8, &[4]),
        ],
        vec![
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
            compute("producer0", vec![], vec![view(&[0], &[2])]),
            tensor("output0", 0x1000, DataType::Int8, &[4]),
            compute("producer1", vec![], vec![view(&[2], &[2])]),
            tensor("output1", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
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
            compute("producer0", vec![], vec![view(&[0], &[1])]),
            tensor("output0", 0x1000, DataType::Int4, &[2]),
            compute("producer1", vec![], vec![view(&[1], &[1])]),
            tensor("output1", 0x1000, DataType::Int4, &[2]),
        ],
        vec![
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
            compute("producer0", vec![], vec![view(&[0, 0], &[2, 2])]),
            tensor("output0", 0x1000, DataType::Int8, &[2, 4]),
            compute("producer1", vec![], vec![view(&[0, 1], &[2, 2])]),
            tensor("output1", 0x1000, DataType::Int8, &[2, 4]),
        ],
        vec![
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
            compute("producer0", vec![], vec![None, None]),
            tensor("gate", 0x2000, DataType::Int8, &[1]),
            tensor("output0", 0x1000, DataType::Int8, &[4]),
            compute("producer1", vec![None], vec![None]),
            tensor("output1", 0x1000, DataType::Int8, &[4]),
        ],
        vec![
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
            compute("reader", vec![None], vec![]),
            compute("writer", vec![], vec![None]),
            tensor("output", 0x1002, DataType::Int8, &[4]),
        ],
        vec![data_edge("input", "reader"), data_edge("writer", "output")],
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
            compute("writer", vec![], vec![None, None]),
            tensor("gate", 0x2000, DataType::Int8, &[1]),
            tensor("output", 0x1000, DataType::Int8, &[4]),
            tensor("input", 0x1000, DataType::Int8, &[4]),
            compute("reader", vec![None, None], vec![]),
        ],
        vec![
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

    let mut nodes = Vec::with_capacity(NUM_PARTITIONS + 1);
    let mut edges = Vec::with_capacity(NUM_PARTITIONS);
    for index in 0..NUM_PARTITIONS {
        nodes.push(compute(
            &format!("producer{index}"),
            vec![],
            vec![view(&[index], &[1])],
        ));
        edges.push(data_edge(&format!("producer{index}"), "output"));
    }
    nodes.push(tensor("output", 0, DataType::Int8, &[NUM_PARTITIONS]));

    timetable(nodes, edges).validate().unwrap();
}

#[test]
fn validates_a_long_chain_of_ordered_alias_writes() {
    const NUM_WRITERS: usize = 1_000;

    let mut nodes = Vec::with_capacity(NUM_WRITERS * 3);
    let mut edges = Vec::with_capacity(NUM_WRITERS * 3);
    for index in 0..NUM_WRITERS {
        let compute_id = format!("producer{index}");
        let gate_id = format!("gate{index}");
        let output_id = format!("output{index}");
        nodes.push(compute(
            &compute_id,
            if index == 0 { vec![] } else { vec![None] },
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
        edges.push(data_edge(&format!("{compute_id}.0"), &gate_id));
        edges.push(data_edge(&format!("{compute_id}.1"), &output_id));
    }

    timetable(nodes, edges).validate().unwrap();
}

#[test]
fn validates_many_interleaved_dependency_chains() {
    const NUM_CHAINS: usize = 64;
    const NUM_STEPS: usize = 64;

    let mut nodes = Vec::with_capacity(NUM_CHAINS * (2 * NUM_STEPS - 1));
    let mut edges = Vec::with_capacity(NUM_CHAINS * 2 * (NUM_STEPS - 1));
    for step in 0..NUM_STEPS {
        for chain in 0..NUM_CHAINS {
            nodes.push(compute(
                &format!("compute{chain}_{step}"),
                if step == 0 { vec![] } else { vec![None] },
                if step + 1 == NUM_STEPS {
                    vec![]
                } else {
                    vec![None]
                },
            ));
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
