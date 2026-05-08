// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gwr_engine::test_helpers::start_test;
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::task::ComputeOp;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::{NodeSection, TensorConfigSection, TimetableFile};

const PLATFORM_YAML: &str = "
memory_maps:
  - name: default
    devices:
      - name: hbm0

processing_elements:
  - name: pe0
    memory_map: default
    config:

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 0x1000_0000
";

fn temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "gwr_gen_timetable_{name}_{}_{}",
        std::process::id(),
        nanos
    ))
}

fn tensor_config<'a>(timetable_file: &'a TimetableFile, id: &str) -> &'a TensorConfigSection {
    timetable_file
        .nodes
        .iter()
        .find_map(|node| match node {
            NodeSection::Tensor {
                id: tensor_id,
                config,
            } if tensor_id == id => Some(config),
            _ => None,
        })
        .unwrap_or_else(|| panic!("tensor {id} not found"))
}

#[test]
fn generator_emits_multi_output_maxpool() {
    let platform_path = temp_path("platform.yaml");
    let timetable_path = temp_path("timetable.yaml");
    fs::write(&platform_path, PLATFORM_YAML).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
        .arg("--platform")
        .arg(&platform_path)
        .arg("--out")
        .arg(&timetable_path)
        .arg("--depth")
        .arg("6")
        .arg("--max-compute-nodes")
        .arg("8")
        .arg("--expand-ratio")
        .arg("1.0")
        .arg("--weight-add")
        .arg("0.0")
        .arg("--weight-gemm")
        .arg("0.0")
        .arg("--weight-maxpool")
        .arg("1.0")
        .arg("--seed")
        .arg("1592612472")
        .arg("--init-rank-min")
        .arg("4")
        .arg("--init-rank-max")
        .arg("4")
        .arg("--init-dim-min")
        .arg("4")
        .arg("--init-dim-max")
        .arg("4")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "gen-timetable failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let timetable_file = TimetableFile::from_file(&timetable_path).unwrap();

    let mut saw_maxpool = false;
    let mut saw_maxpool_indices = false;

    for node in &timetable_file.nodes {
        let NodeSection::Compute {
            id,
            op,
            output_views,
            ..
        } = node
        else {
            continue;
        };

        match op {
            ComputeOp::Add | ComputeOp::Gemm => {}
            ComputeOp::MaxPool(_) => {
                saw_maxpool = true;
                assert!((1..=2).contains(&output_views.len()));
                if output_views.len() == 1 {
                    continue;
                }
                saw_maxpool_indices = true;

                let values_edge = timetable_file
                    .edges
                    .iter()
                    .find(|edge| edge.from == format!("{id}.0"))
                    .unwrap_or_else(|| panic!("missing values edge for {id}"));
                let indices_edge = timetable_file
                    .edges
                    .iter()
                    .find(|edge| edge.from == format!("{id}.1"))
                    .unwrap_or_else(|| panic!("missing indices edge for {id}"));

                let values = tensor_config(&timetable_file, &values_edge.to);
                let indices = tensor_config(&timetable_file, &indices_edge.to);
                assert_eq!(indices.dtype, DataType::Int64);
                assert_eq!(indices.shape, values.shape);
            }
        }
    }

    assert!(saw_maxpool, "generated timetable did not contain MaxPool");
    assert!(
        saw_maxpool_indices,
        "generated timetable did not contain MaxPool indices output"
    );

    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, PLATFORM_YAML).unwrap());
    Timetable::new(&engine.top().clone(), timetable_file, &platform).unwrap();

    let _ = fs::remove_file(platform_path);
    let _ = fs::remove_file(timetable_path);
}
