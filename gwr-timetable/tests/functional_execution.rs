// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Independent numeric checks through the ordinary simulated memory path.
#![cfg(feature = "functional")]

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::memory::FunctionalMemory;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;

const A: [f32; 10] = [1., -2., 3., 0., 4., -1., 2., 0., 5., -3.];
const B: [f32; 15] = [
    2., -1., 0., 3., 4., -2., -1., 2., 5., 6., 0., -3., 0., -2., 1.,
];
// Independently evaluated 2x5 by 5x3 product, in row-major order.
const PRODUCT: [f32; 6] = [-7., -11., 23., 34., 15., -22.];
const BIAS: [f32; 6] = [0.5, 2., -4., -3., 1., 8.];

const PLATFORM: &str = r"
memory_maps:
  - name: mm
    devices: [mem]
operation_classes:
  matrix: { operations: [matrix_mac], arity: 2, flops_per_op: 2 }
  add: { operations: [add], arity: 2 }
  compare: { operations: [compare, maximum], arity: 2 }
  mul: { operations: [mul], arity: 2 }
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      sram_bytes: 256
      compute:
        engines:
          matrix:
            capabilities: [{ class: matrix, input_dtypes: [[fp32, fp32]], ops_per_tick: 10 }]
          vector:
            capabilities: [{ class: add, input_dtypes: [[fp32, fp32]], ops_per_tick: 2 }, { class: compare, input_dtypes: [[fp32, fp32]], ops_per_tick: 2 }, { class: mul, input_dtypes: [[fp32, fp32]], ops_per_tick: 2 }]
memories:
  - name: mem
    kind: hbm
    base_address: 0
    config: { capacity_bytes: 1MiB }
connections:
  - connect: [pe.pe0, mem.mem]
";

fn source(transpose_b: bool, chain: bool) -> String {
    let (shape, indices) = if transpose_b {
        ("[3, 5]", "[n, k]")
    } else {
        ("[5, 3]", "[k, n]")
    };
    let mut source = format!(
        r"
nodes:
  - {{ id: A, kind: tensor, config: {{ addr: 0x1000, dtype: fp32, shape: [2, 5] }} }}
  - {{ id: B, kind: tensor, config: {{ addr: 0x2000, dtype: fp32, shape: {shape} }} }}
  - id: gemm
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [m, n]
          expr: {{ matrix_mac: {{ over: [k], args: [{{ read: {{ tensor: A, indices: [m, k] }} }}, {{ read: {{ tensor: B, indices: {indices} }} }}] }} }}
    tiling: {{ sizes: {{ m: 1, n: 2, k: 2 }} }}
  - {{ id: Y, kind: tensor, config: {{ addr: 0x3000, dtype: fp32, shape: [2, 3] }} }}
edges:
  - {{ from: A, to: gemm.0, kind: data }}
  - {{ from: B, to: gemm.1, kind: data }}
  - {{ from: gemm.0, to: Y, kind: data }}
"
    );
    if chain {
        source = source.replace("edges:", r"
  - { id: copy, kind: transfer, pe: pe0, buffer_bytes: 8 }
  - { id: copied, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [2, 3] } }
  - { id: bias, kind: tensor, config: { addr: 0x5000, dtype: fp32, shape: [2, 3] } }
  - id: consumer
    kind: compute
    pe: pe0
    program:
      inputs: [copied, bias]
      outputs: [Z]
      assignments:
        - output: Z
          axes: [m, n]
          expr: { add: [{ read: { tensor: copied, indices: [m, n] } }, { read: { tensor: bias, indices: [m, n] } }] }
    tiling: { sizes: { m: 1, n: 2 } }
  - { id: Z, kind: tensor, config: { addr: 0x6000, dtype: fp32, shape: [2, 3] } }
edges:
  - { from: Y, to: copy, kind: data }
  - { from: copy, to: copied, kind: data }
  - { from: copied, to: consumer.0, kind: data }
  - { from: bias, to: consumer.1, kind: data }
  - { from: consumer.0, to: Z, kind: data }
");
    }
    source
}

#[derive(Debug, PartialEq)]
struct Timing {
    ticks: u64,
    traffic: (usize, usize),
    peak_sram: usize,
    operations: usize,
}

fn run(
    source: &str,
    memory: Option<Rc<FunctionalMemory>>,
) -> Result<Timing, gwr_engine::types::SimError> {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker;
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, PLATFORM)?);
    let pe = platform.pe("pe0")?;
    if let Some(memory) = memory {
        pe.enable_functional(memory)?;
    }
    let file = TimetableFile::from_string(source)?;
    let timetable = Rc::new(Timetable::new(engine.top(), file.into_graph()?, &platform)?);
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run()?;
    timetable.check_tasks_complete()?;
    Ok(Timing {
        ticks: clock.tick_now().tick(),
        traffic: pe.memory_traffic_bytes(),
        peak_sram: pe.peak_sram_bytes(),
        operations: timetable.machine_operations_executed()?,
    })
}

fn initialized(transpose_b: bool) -> Rc<FunctionalMemory> {
    let memory = Rc::new(FunctionalMemory::default());
    memory.write_f32(0x1000, &A).unwrap();
    let values = if transpose_b {
        (0..3)
            .flat_map(|column| (0..5).map(move |row| B[row * 3 + column]))
            .collect::<Vec<_>>()
    } else {
        B.to_vec()
    };
    memory.write_f32(0x2000, &values).unwrap();
    memory.write_f32(0x5000, &BIAS).unwrap();
    memory
}

#[test]
fn asymmetric_gemm_matches_independent_reference_with_ragged_and_reduction_tiles() {
    for transpose_b in [false, true] {
        let source = source(transpose_b, false);
        let memory = initialized(transpose_b);
        assert!(memory.read_f32(0x3000, 6).is_err());
        let functional = run(&source, Some(memory.clone())).unwrap();
        assert_eq!(memory.read_f32(0x3000, 6).unwrap(), PRODUCT);
        // The optional arithmetic must preserve scheduling, transfers and SRAM.
        assert_eq!(functional, run(&source, None).unwrap());
        assert_eq!(functional.operations, 30);
    }
}

#[test]
fn gemm_handles_all_operand_transposes_without_changing_simulated_work() {
    for transpose_a in [false, true] {
        for transpose_b in [false, true] {
            let mut program = source(transpose_b, false);
            let memory = initialized(transpose_b);
            if transpose_a {
                program = program
                    .replace("shape: [2, 5]", "shape: [5, 2]")
                    .replace("tensor: A, indices: [m, k]", "tensor: A, indices: [k, m]");
                let a = (0..5)
                    .flat_map(|k| (0..2).map(move |m| A[m * 5 + k]))
                    .collect::<Vec<_>>();
                memory.write_f32(0x1000, &a).unwrap();
            }
            assert_eq!(
                run(&program, Some(memory.clone())).unwrap(),
                run(&program, None).unwrap()
            );
            assert_eq!(memory.read_f32(0x3000, 6).unwrap(), PRODUCT);
        }
    }
}

#[test]
fn gemm_packs_offset_strided_views_and_preserves_output_neighbors() {
    let program = source(false, false)
        .replace("shape: [2, 5]", "shape: [4, 8]")
        .replace("shape: [5, 3]", "shape: [7, 6]")
        .replace("shape: [2, 3]", "shape: [4, 6]")
        .replace("    pe: pe0\n", "    pe: pe0\n    input_views: [{offsets: [1, 2], shape: [2, 5]}, {offsets: [1, 2], shape: [5, 3]}]\n    output_views: [{offsets: [1, 1], shape: [2, 3]}]\n");
    let mut a = vec![99.0; 32];
    let mut b = vec![99.0; 42];
    for row in 0..2 {
        a[(row + 1) * 8 + 2..(row + 1) * 8 + 7].copy_from_slice(&A[row * 5..(row + 1) * 5]);
    }
    for row in 0..5 {
        b[(row + 1) * 6 + 2..(row + 1) * 6 + 5].copy_from_slice(&B[row * 3..(row + 1) * 3]);
    }
    let memory = Rc::new(FunctionalMemory::default());
    memory.write_f32(0x1000, &a).unwrap();
    memory.write_f32(0x2000, &b).unwrap();
    memory.write_f32(0x3000, &[77.0; 24]).unwrap();
    assert_eq!(
        run(&program, Some(memory.clone())).unwrap(),
        run(&program, None).unwrap()
    );
    let mut expected = vec![77.0; 24];
    expected[7..10].copy_from_slice(&PRODUCT[..3]);
    expected[13..16].copy_from_slice(&PRODUCT[3..]);
    assert_eq!(memory.read_f32(0x3000, 24).unwrap(), expected);
}

#[test]
fn gemm_singleton_rows_broadcast_in_both_physical_orientations() {
    for transposed in [false, true] {
        let mut program = source(false, false).replace(
            "      outputs: [Y]\n",
            "      outputs: [Y]\n      extents: {m: 2}\n",
        );
        program = program.replace(
            "shape: [2, 5]",
            if transposed {
                "shape: [5, 1]"
            } else {
                "shape: [1, 5]"
            },
        );
        if transposed {
            program = program.replace("tensor: A, indices: [m, k]", "tensor: A, indices: [k, m]");
        }
        let memory = initialized(false);
        memory.write_f32(0x1000, &A[..5]).unwrap();
        assert_eq!(
            run(&program, Some(memory.clone())).unwrap(),
            run(&program, None).unwrap()
        );
        assert_eq!(
            memory.read_f32(0x3000, 6).unwrap(),
            [-7., -11., 23., -7., -11., 23.]
        );
    }
}

#[test]
fn sum_of_products_uses_the_same_dense_contraction_semantics() {
    let mut document: serde_json::Value = yaml_serde::from_str(&source(false, false)).unwrap();
    let node = document["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == "gemm")
        .unwrap();
    let mac = node["program"]["assignments"][0]["expr"]["matrix_mac"].take();
    node["program"]["assignments"][0]["expr"] = serde_json::json!({"reduce": {
        "op": "sum", "over": mac["over"], "value": {"mul": mac["args"]},
    }});
    let program = document.to_string();
    let memory = initialized(false);
    assert_eq!(
        run(&program, Some(memory.clone())).unwrap(),
        run(&program, None).unwrap()
    );
    assert_eq!(memory.read_f32(0x3000, 6).unwrap(), PRODUCT);
}

#[test]
fn permuted_gemm_output_axes_keep_scalar_fallback_correct() {
    let program = source(false, false)
        .replace("axes: [m, n]", "axes: [n, m]")
        .replace("shape: [2, 3]", "shape: [3, 2]");
    let memory = initialized(false);
    assert_eq!(
        run(&program, Some(memory.clone())).unwrap(),
        run(&program, None).unwrap()
    );
    assert_eq!(
        memory.read_f32(0x3000, 6).unwrap(),
        [-7., 34., -11., 15., 23., -22.]
    );
}

#[test]
fn producer_transfer_consumer_preserves_values_between_distinct_addresses() {
    let source = source(false, true);
    let memory = initialized(false);
    let functional = run(&source, Some(memory.clone())).unwrap();
    assert_eq!(memory.read_f32(0x3000, 6).unwrap(), PRODUCT);
    assert_eq!(memory.read_f32(0x4000, 6).unwrap(), PRODUCT);
    assert_eq!(
        memory.read_f32(0x6000, 6).unwrap(),
        [-6.5, -9., 19., 31., 16., -14.]
    );
    assert_eq!(functional, run(&source, None).unwrap());
    assert_eq!(functional.operations, 36);
}

#[test]
fn missing_operand_data_fails_instead_of_silently_reading_zero() {
    let memory = Rc::new(FunctionalMemory::default());
    memory.write_f32(0x1000, &A).unwrap();
    let error = run(&source(false, false), Some(memory)).unwrap_err();
    assert!(error.to_string().contains("uninitialized"), "{error}");
}

#[test]
fn partially_initialized_operand_fails_even_when_first_tile_is_valid() {
    let memory = Rc::new(FunctionalMemory::default());
    memory.write_f32(0x1000, &A).unwrap();
    memory.write_f32(0x2000, &B[..14]).unwrap();
    let error = run(&source(false, false), Some(memory)).unwrap_err();
    assert!(error.to_string().contains("uninitialized"), "{error}");
}

#[test]
fn strided_subviews_preserve_unwritten_bytes_in_shared_memory_packets() {
    let source = r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [4, 5] } }
  - id: view_copy
    kind: compute
    pe: pe0
    input_views: [{ offsets: [1, 1], shape: [2, 3] }]
    output_views: [{ offsets: [1, 1], shape: [2, 3] }]
    program:
      inputs: [A]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i, j]
          expr: { read: { tensor: A, indices: [i, j] } }
    tiling: { sizes: { i: 1, j: 1 } }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [4, 5] } }
edges:
  - { from: A, to: view_copy.0, kind: data }
  - { from: view_copy.0, to: Y, kind: data }
";
    let memory = Rc::new(FunctionalMemory::default());
    let input = (0..20).map(|value| value as f32 - 7.).collect::<Vec<_>>();
    memory.write_f32(0x1000, &input).unwrap();
    memory.write_f32(0x3000, &[99.; 20]).unwrap();
    run(source, Some(memory.clone())).unwrap();
    let mut expected = vec![99.; 20];
    for index in [6, 7, 8, 11, 12, 13] {
        expected[index] = input[index];
    }
    assert_eq!(memory.read_f32(0x3000, 20).unwrap(), expected);
}

#[test]
fn transfer_buffer_smaller_than_one_float_preserves_each_byte() {
    let source = r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [5] } }
  - { id: copy, kind: transfer, pe: pe0, buffer_bytes: 1 }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [5] } }
edges:
  - { from: A, to: copy, kind: data }
  - { from: copy, to: Y, kind: data }
";
    let memory = Rc::new(FunctionalMemory::default());
    let values = [f32::from_bits(1), -0., 1.25, -923.5, f32::INFINITY];
    memory.write_f32(0x1000, &values).unwrap();
    run(source, Some(memory.clone())).unwrap();
    assert_eq!(
        memory
            .read_f32(0x3000, 5)
            .unwrap()
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>(),
        values.map(f32::to_bits)
    );
}

#[test]
fn fractional_gemm_agrees_with_independent_f64_reference() {
    let a = [0.1_f32, -0.7, 1.25, 3.3, -0.01, -2.2, 0.4, 0.125, -1.7, 4.1];
    let b = [
        0.3_f32, -1.1, 0.7, 2.25, 0.2, -0.8, -0.33, 0.4, 1.9, 1.1, -0.6, 0.025, 0.45, 1.75, -2.3,
    ];
    // Accumulate exact loaded FP32 operands in f64, independently of the
    // simulated evaluator's FP32 partial reductions and tile order.
    let expected = (0..2)
        .flat_map(|row| {
            (0..3).map(move |column| {
                (0..5)
                    .map(|k| f64::from(a[row * 5 + k]) * f64::from(b[k * 3 + column]))
                    .sum::<f64>()
            })
        })
        .collect::<Vec<_>>();
    let memory = Rc::new(FunctionalMemory::default());
    memory.write_f32(0x1000, &a).unwrap();
    memory.write_f32(0x2000, &b).unwrap();
    run(&source(false, false), Some(memory.clone())).unwrap();
    for (index, (&actual, &expected)) in memory
        .read_f32(0x3000, 6)
        .unwrap()
        .iter()
        .zip(&expected)
        .enumerate()
    {
        let error = (f64::from(actual) - expected).abs();
        assert!(
            error <= 2e-6 + 2e-6 * expected.abs(),
            "element {index}: actual {actual}, f64 reference {expected}, error {error}"
        );
    }
}

#[test]
fn unsupported_boolean_matrix_fails_before_memory_traffic_or_output_writes() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker;
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    // A timing capability can declare Bool operands; numerical matrix
    // accumulation requires an explicitly supported arithmetic domain.
    let platform =
        Rc::new(Platform::from_string(&engine, &clock, &PLATFORM.replace("fp32", "bool")).unwrap());
    let memory = Rc::new(FunctionalMemory::default());
    memory.write_f32(0x3000, &[77.; 6]).unwrap();
    let pe = platform.pe("pe0").unwrap();
    pe.enable_functional(memory.clone()).unwrap();
    let file = TimetableFile::from_string(&source(false, false).replace("fp32", "bool")).unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    let error = engine.run().unwrap_err();
    assert!(error.to_string().contains("Bool"), "{error}");
    assert_eq!(pe.memory_traffic_bytes(), (0, 0));
    assert_eq!(memory.read_f32(0x3000, 6).unwrap(), [77.; 6]);
}

#[test]
fn padded_strided_windows_exclude_padding_from_negative_max() {
    let source = r"
nodes:
  - { id: A, kind: tensor, config: { addr: 4096, dtype: fp32, shape: [4] } }
  - id: pool
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y]
      extents: { y: 2, k: 3 }
      assignments:
        - output: Y
          axes: [y]
          expr:
            reduce:
              op: max
              over: [k]
              value: { read: { tensor: A, indices: [{ affine: { terms: { y: 2, k: 1 }, offset: -1 } }], bounds: exclude } }
    tiling: { sizes: { y: 1, k: 2 } }
  - { id: Y, kind: tensor, config: { addr: 12288, dtype: fp32, shape: [2] } }
edges:
  - { from: A, to: pool, kind: data }
  - { from: pool, to: Y, kind: data }
";
    let memory = Rc::new(FunctionalMemory::default());
    memory.write_f32(4096, &[-5., -4., -3., -2.]).unwrap();
    run(source, Some(memory.clone())).unwrap();
    assert_eq!(memory.read_f32(12288, 2).unwrap(), [-4., -2.]);
}

#[test]
fn asymmetric_convolution_matches_independent_reference_with_padding_stride_and_tiles() {
    let source = CONVOLUTION_SOURCE;
    let input = (0..40)
        .map(|i| ((i * 7 % 31) as f32 - 15.) / 7.)
        .collect::<Vec<_>>();
    let weights = (0..36)
        .map(|i| ((i * 3 % 17) as f32 - 8.) / 9.)
        .collect::<Vec<_>>();
    let mut expected = Vec::new();
    for oc in 0..2 {
        for oy in 0..2 {
            for ox in 0..3 {
                let mut sum = 0.0f64;
                for ic in 0..2 {
                    for ky in 0..3 {
                        for kx in 0..3 {
                            let iy = oy * 2 + ky - 1_i32;
                            let ix = ox * 2 + kx - 1_i32;
                            if (0..4).contains(&iy) && (0..5).contains(&ix) {
                                sum += f64::from(input[ic * 20 + iy as usize * 5 + ix as usize])
                                    * f64::from(
                                        weights
                                            [((oc * 2 + ic) * 3 + ky as usize) * 3 + kx as usize],
                                    );
                            }
                        }
                    }
                }
                expected.push(sum as f32);
            }
        }
    }
    let memory = Rc::new(FunctionalMemory::default());
    memory.write_f32(4096, &input).unwrap();
    memory.write_f32(8192, &weights).unwrap();
    let functional = run(source, Some(memory.clone())).unwrap();
    memory.check_f32(12288, &expected, 2e-6, 2e-6).unwrap();
    assert_eq!(functional, run(source, None).unwrap());
}

const CONVOLUTION_SOURCE: &str = r"
nodes:
  - { id: A, kind: tensor, config: { addr: 4096, dtype: fp32, shape: [1, 2, 4, 5] } }
  - { id: B, kind: tensor, config: { addr: 8192, dtype: fp32, shape: [2, 2, 3, 3] } }
  - id: conv
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      extents: { y: 2, x: 3, kh: 3, kw: 3 }
      assignments:
        - output: Y
          axes: [b, o, y, x]
          expr:
            reduce:
              op: sum
              over: [c, kh, kw]
              value:
                mul:
                  - { read: { tensor: A, indices: [b, c, { affine: { terms: { y: 2, kh: 1 }, offset: -1 } }, { affine: { terms: { x: 2, kw: 1 }, offset: -1 } }], bounds: exclude } }
                  - { read: { tensor: B, indices: [o, c, kh, kw] } }
    tiling: { sizes: { b: 1, o: 1, y: 1, x: 2, c: 1, kh: 2, kw: 2 } }
  - { id: Y, kind: tensor, config: { addr: 12288, dtype: fp32, shape: [1, 2, 2, 3] } }
edges:
  - { from: A, to: conv.0, kind: data }
  - { from: B, to: conv.1, kind: data }
  - { from: conv.0, to: Y, kind: data }
";

#[test]
fn convolution_broadcast_fallback_matches_scalar_semantics() {
    let source = CONVOLUTION_SOURCE.replace("shape: [1, 2, 4, 5]", "shape: [1, 1, 4, 5]");
    let memory = Rc::new(FunctionalMemory::default());
    memory.write_f32(4096, &[1.; 20]).unwrap();
    let weights = (0..36).map(|i| (i + 1) as f32).collect::<Vec<_>>();
    memory.write_f32(8192, &weights).unwrap();
    run(&source, Some(memory.clone())).unwrap();
    // Top-left contribution selects kh,kw in1..3; all other positions
    // independently enumerate the valid kernel cells for two broadcast
    // channels.
    let mut expected = Vec::new();
    for oc in 0..2 {
        for oy in 0..2 {
            for ox in 0..3 {
                let mut sum = 0.;
                for ic in 0..2 {
                    for ky in 0..3 {
                        for kx in 0..3 {
                            let y = oy * 2 + ky - 1_i32;
                            let x = ox * 2 + kx - 1_i32;
                            if (0..4).contains(&y) && (0..5).contains(&x) {
                                sum += weights[((oc * 2 + ic) * 3 + ky as usize) * 3 + kx as usize];
                            }
                        }
                    }
                }
                expected.push(sum);
            }
        }
    }
    assert_eq!(memory.read_f32(12288, 12).unwrap(), expected);
}

#[test]
fn convolution_empty_windows_sum_to_zero() {
    let source = CONVOLUTION_SOURCE.replace("offset: -1", "offset: 10");
    let memory = Rc::new(FunctionalMemory::default());
    memory.write_f32(4096, &[1.; 40]).unwrap();
    memory.write_f32(8192, &[2.; 36]).unwrap();
    run(&source, Some(memory.clone())).unwrap();
    assert_eq!(memory.read_f32(12288, 12).unwrap(), [0.; 12]);
}

#[test]
fn overlapping_convolution_inputs_and_outputs_are_rejected_before_execution() {
    let source = CONVOLUTION_SOURCE
        .replace("addr: 12288", "addr: 4096")
        .replace(
            "b: 1, o: 1, y: 1, x: 2, c: 1, kh: 2, kw: 2",
            "b: 1, o: 2, y: 2, x: 3, c: 2, kh: 3, kw: 3",
        );
    let memory = Rc::new(FunctionalMemory::default());
    memory.write_f32(4096, &[1.; 40]).unwrap();
    memory.write_f32(8192, &[1.; 36]).unwrap();
    let error = run(&source, Some(memory.clone())).unwrap_err();
    assert!(error.to_string().contains("overlapping range"), "{error}");
    assert_eq!(memory.read_f32(4096, 40).unwrap(), [1.; 40]);
}

#[test]
fn singleton_spatial_affine_indices_still_exclude_padding() {
    let source = CONVOLUTION_SOURCE
        .replace("shape: [1, 2, 4, 5]", "shape: [1, 2, 1, 1]")
        .replace("shape: [1, 2, 2, 3]", "shape: [1, 2, 1, 1]")
        .replace("y: 2, x: 3, kh: 3", "y: 1, x: 1, kh: 3");
    let memory = Rc::new(FunctionalMemory::default());
    memory.write_f32(4096, &[2., -3.]).unwrap();
    let weights = (0..36).map(|i| i as f32 + 1.).collect::<Vec<_>>();
    memory.write_f32(8192, &weights).unwrap();
    run(&source, Some(memory.clone())).unwrap();
    assert_eq!(
        memory.read_f32(12288, 2).unwrap(),
        [
            2. * weights[4] - 3. * weights[13],
            2. * weights[22] - 3. * weights[31]
        ]
    );
}
