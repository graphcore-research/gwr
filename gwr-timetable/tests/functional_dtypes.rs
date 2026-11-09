// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Typed arithmetic checks through the actual PE, LSU and backing memory.
#![cfg(feature = "functional")]

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::types::SimError;
use gwr_models::memory::{FunctionalMemory, FunctionalValue as V};
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::tensors::dtype::DataType as D;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;

fn name(dtype: D) -> String {
    format!("{dtype:?}").to_lowercase()
}

fn platform(operation: &str, inputs: &[D], accumulator: Option<D>) -> String {
    let instruction = accumulator.map_or_else(String::new, |dtype| {
        format!(
            ", execution: {{ instruction: tensor, accumulator: {}, sparsity: dense }}",
            name(dtype)
        )
    });
    let inputs = inputs
        .iter()
        .map(|dtype| name(*dtype))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r"
memory_maps:
  - {{ name: mm, devices: [mem] }}
operation_classes:
  op: {{ operations: [{operation}], arity: {} }}
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      sram_bytes: 4096
      compute:
        engines:
          engine:
            capabilities: [{{ class: op, input_dtypes: [[{inputs}]], ops_per_tick: 8{instruction} }}]
memories:
  - name: mem
    kind: hbm
    base_address: 0
    config: {{ capacity_bytes: 1MiB }}
connections:
  - connect: [pe.pe0, mem.mem]
",
        inputs.split(',').count()
    )
}

#[derive(Debug, PartialEq, Eq)]
struct Timing {
    ticks: u64,
    traffic: (usize, usize),
    operations: usize,
}

fn run(
    platform: &str,
    source: &str,
    memory: Option<Rc<FunctionalMemory>>,
) -> Result<Timing, SimError> {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, platform)?);
    let pe = platform.pe("pe0")?;
    if let Some(memory) = memory {
        pe.enable_functional(memory)?;
    }
    let timetable = Rc::new(Timetable::new(
        engine.top(),
        TimetableFile::from_string(source)?.into_graph()?,
        &platform,
    )?);
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run()?;
    timetable.check_tasks_complete()?;
    Ok(Timing {
        ticks: clock.tick_now().tick(),
        traffic: pe.memory_traffic_bytes(),
        operations: timetable.machine_operations_executed()?,
    })
}

fn scalar(input: D, output: D, count: usize, expr: &str, binary: bool) -> String {
    let b = if binary {
        format!(
            "  - {{ id: B, kind: tensor, config: {{ addr: 0x2000, dtype: {}, shape: [{count}] }} }}",
            name(input)
        )
    } else {
        String::new()
    };
    let b_edge = if binary {
        "  - { from: B, to: compute.1, kind: data }"
    } else {
        ""
    };
    let inputs = if binary { "A, B" } else { "A" };
    format!(
        r"
nodes:
  - {{ id: A, kind: tensor, config: {{ addr: 0x1000, dtype: {}, shape: [{count}] }} }}
{b}
  - id: compute
    kind: compute
    pe: pe0
    program:
      inputs: [{inputs}]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr: {expr}
    tiling: {{ sizes: {{ i: 1 }} }}
  - {{ id: Y, kind: tensor, config: {{ addr: 0x3000, dtype: {}, shape: [{count}] }} }}
edges:
  - {{ from: A, to: compute.0, kind: data }}
{b_edge}
  - {{ from: compute.0, to: Y, kind: data }}
",
        name(input),
        name(output)
    )
}

const READ: &str = "{ read: { tensor: A, indices: [i] } }";
const ADD: &str =
    "{ add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }";

#[test]
fn every_dtype_copies_exact_bits_in_single_element_tiles() {
    for dtype in D::ALL {
        let mask = u64::MAX >> (64 - dtype.num_bits());
        let values = [0, 1, mask, mask / 3, mask / 2, 0, mask, 1, 0]
            .map(|bits| V::from_bits(dtype, bits).unwrap());
        let memory = Rc::new(FunctionalMemory::default());
        memory.write_values(0x1000, dtype, &values).unwrap();
        let source = scalar(dtype, dtype, values.len(), READ, false);
        let platform = platform("add", &[dtype, dtype], None);
        let functional = run(&platform, &source, Some(memory.clone())).unwrap();
        memory.check_values(0x3000, dtype, &values).unwrap();
        assert_eq!(
            functional,
            run(&platform, &source, None).unwrap(),
            "{dtype:?}"
        );
    }
}

#[test]
fn packed_subviews_preserve_bits_outside_the_output() {
    for dtype in [D::Bool, D::Int4, D::Fp4] {
        let count = if dtype == D::Bool { 16 } else { 4 };
        let mask = u64::MAX >> (64 - dtype.num_bits());
        let input = (0..count)
            .map(|i| V::from_bits(dtype, (i as u64) & mask).unwrap())
            .collect::<Vec<_>>();
        let sentinel = V::from_bits(dtype, mask).unwrap();
        let memory = Rc::new(FunctionalMemory::default());
        memory.write_values(0x1000, dtype, &input).unwrap();
        memory
            .write_values(0x3000, dtype, &vec![sentinel; count])
            .unwrap();
        let source = scalar(dtype, dtype, count, READ, false).replace(
            "    program:",
            &format!("    input_views: [{{ offsets: [1], shape: [{}] }}]\n    output_views: [{{ offsets: [1], shape: [{}] }}]\n    program:", count - 2, count - 2),
        );
        run(
            &platform("add", &[dtype, dtype], None),
            &source,
            Some(memory.clone()),
        )
        .unwrap();
        let mut expected = input;
        expected[0] = sentinel;
        expected[count - 1] = sentinel;
        memory.check_values(0x3000, dtype, &expected).unwrap();
    }
}

#[test]
fn dma_transfers_packed_logical_values_with_uninitialized_tail_padding() {
    for (dtype, count) in [(D::Bool, 9), (D::Int4, 3), (D::Fp4, 3)] {
        let memory = Rc::new(FunctionalMemory::default());
        let values = (0..count)
            .map(|i| V::from_bits(dtype, (i % 2) as u64).unwrap())
            .collect::<Vec<_>>();
        memory.write_values(0x1000, dtype, &values).unwrap();
        let source = format!(
            r"
nodes:
  - {{ id: A, kind: tensor, config: {{ addr: 0x1000, dtype: {}, shape: [{count}] }} }}
  - {{ id: copy, kind: transfer, pe: pe0, buffer_bytes: 1 }}
  - {{ id: Y, kind: tensor, config: {{ addr: 0x3000, dtype: {}, shape: [{count}] }} }}
edges:
  - {{ from: A, to: copy, kind: data }}
  - {{ from: copy, to: Y, kind: data }}
",
            name(dtype),
            name(dtype)
        );
        let platform = platform("add", &[dtype, dtype], None);
        let functional = run(&platform, &source, Some(memory.clone())).unwrap();
        memory.check_values(0x3000, dtype, &values).unwrap();
        assert_eq!(functional, run(&platform, &source, None).unwrap());
    }
}

fn integer(dtype: D, value: i64) -> V {
    if matches!(dtype, D::Int64 | D::Int32 | D::Int16 | D::Int8 | D::Int4) {
        V::from_i64(dtype, value).unwrap()
    } else {
        V::from_u64(dtype, value as u64).unwrap()
    }
}

#[test]
fn addition_computes_for_every_numeric_dtype() {
    for dtype in D::ALL.into_iter().filter(|dtype| *dtype != D::Bool) {
        let floating = matches!(dtype, D::Fp32 | D::Bf16 | D::Fp16 | D::Fp8 | D::Fp4);
        let values = [1, 2, 3].map(|value| {
            if floating {
                V::from_f32(dtype, value as f32).unwrap()
            } else {
                integer(dtype, value)
            }
        });
        let memory = Rc::new(FunctionalMemory::default());
        memory.write_values(0x1000, dtype, &values[..1]).unwrap();
        memory.write_values(0x2000, dtype, &values[1..2]).unwrap();
        let source = scalar(dtype, dtype, 1, ADD, true);
        let platform = platform("add", &[dtype, dtype], None);
        let functional = run(&platform, &source, Some(memory.clone())).unwrap();
        memory.check_values(0x3000, dtype, &values[2..]).unwrap();
        assert_eq!(functional, run(&platform, &source, None).unwrap());
    }
}

#[test]
fn boolean_logic_and_wide_integer_comparison_produce_packed_results() {
    let memory = Rc::new(FunctionalMemory::default());
    let booleans = [true, false, true].map(V::from_bool);
    memory.write_values(0x1000, D::Bool, &booleans).unwrap();
    memory
        .write_values(0x2000, D::Bool, &[V::from_bool(true); 3])
        .unwrap();
    let source = scalar(D::Bool, D::Bool, 3, &ADD.replace("add:", "bitand:"), true);
    run(
        &platform("bitand", &[D::Bool, D::Bool], None),
        &source,
        Some(memory.clone()),
    )
    .unwrap();
    memory.check_values(0x3000, D::Bool, &booleans).unwrap();

    let values = [u64::MAX, (1 << 53) + 1, 0].map(|value| V::from_u64(D::Uint64, value).unwrap());
    memory.write_values(0x1000, D::Uint64, &values).unwrap();
    memory
        .write_values(
            0x2000,
            D::Uint64,
            &[V::from_u64(D::Uint64, 1 << 53).unwrap(); 3],
        )
        .unwrap();
    let source = scalar(
        D::Uint64,
        D::Bool,
        3,
        &ADD.replace("add:", "greater:"),
        true,
    );
    run(
        &platform("greater", &[D::Uint64, D::Uint64], None),
        &source,
        Some(memory.clone()),
    )
    .unwrap();
    memory
        .check_values(0x3000, D::Bool, &[true, true, false].map(V::from_bool))
        .unwrap();
}

#[test]
fn reductions_compute_for_each_numeric_dtype() {
    for dtype in D::ALL.into_iter().filter(|dtype| *dtype != D::Bool) {
        let encode = |value: i64| {
            if matches!(dtype, D::Fp32 | D::Fp16 | D::Bf16 | D::Fp8 | D::Fp4) {
                V::from_f32(dtype, value as f32).unwrap()
            } else {
                integer(dtype, value)
            }
        };
        let memory = Rc::new(FunctionalMemory::default());
        memory
            .write_values(0x1000, dtype, &[1, 2, 1, 2].map(encode))
            .unwrap();
        let source = scalar(
            dtype,
            dtype,
            1,
            "{ reduce: { op: sum, over: [k], value: { read: { tensor: A, indices: [k] } } } }",
            false,
        )
        .replacen("shape: [1]", "shape: [4]", 1)
        .replace("inputs: [A]", "inputs: [A]\n      extents: { i: 1 }");
        let platform = platform("add", &[dtype, dtype], None);
        let functional = run(&platform, &source, Some(memory.clone())).unwrap();
        memory.check_values(0x3000, dtype, &[encode(6)]).unwrap();
        assert_eq!(functional, run(&platform, &source, None).unwrap());
    }
}

#[test]
fn extrema_compare_in_source_precision_before_narrowing_the_winner() {
    let memory = Rc::new(FunctionalMemory::default());
    memory
        .write_values(
            0x1000,
            D::Int64,
            &[256, 127].map(|value| integer(D::Int64, value)),
        )
        .unwrap();
    for (operation, expected) in [("min", 127), ("max", 0)] {
        let source = scalar(D::Int64, D::Int8, 1,
            &format!("{{ reduce: {{ op: {operation}, over: [k], value: {{ read: {{ tensor: A, indices: [k] }} }} }} }}"), false)
            .replacen("shape: [1]", "shape: [2]", 1)
            .replace("inputs: [A]", "inputs: [A]\n      extents: { i: 1 }");
        let operation = if operation == "min" {
            "compare, minimum"
        } else {
            "compare, maximum"
        };
        run(
            &platform(operation, &[D::Int64, D::Int64], None),
            &source,
            Some(memory.clone()),
        )
        .unwrap();
        memory
            .check_values(0x3000, D::Int8, &[integer(D::Int8, expected)])
            .unwrap();
    }
}

#[test]
fn wide_integer_arithmetic_does_not_round_through_floating_point() {
    for (dtype, input, expected) in [
        (
            D::Uint64,
            vec![9_007_199_254_740_993, u64::MAX, 1 << 63],
            vec![9_007_199_254_740_994, 0, (1 << 63) + 1],
        ),
        (
            D::Int64,
            vec![
                i64::MAX as u64,
                i64::MIN as u64,
                (-9_007_199_254_740_993_i64) as u64,
            ],
            vec![
                i64::MIN as u64,
                (i64::MIN + 1) as u64,
                (-9_007_199_254_740_992_i64) as u64,
            ],
        ),
    ] {
        let memory = Rc::new(FunctionalMemory::default());
        let raw = |values: &[u64]| {
            values
                .iter()
                .map(|bits| V::from_bits(dtype, *bits).unwrap())
                .collect::<Vec<_>>()
        };
        memory.write_values(0x1000, dtype, &raw(&input)).unwrap();
        memory
            .write_values(0x2000, dtype, &[V::from_bits(dtype, 1).unwrap(); 3])
            .unwrap();
        run(
            &platform("add", &[dtype, dtype], None),
            &scalar(dtype, dtype, 3, ADD, true),
            Some(memory.clone()),
        )
        .unwrap();
        memory.check_values(0x3000, dtype, &raw(&expected)).unwrap();
    }
}

#[test]
fn integer_conversion_rounds_once_at_the_destination_precision() {
    for (output, input, expected_bits) in [
        (D::Fp32, (1_u64 << 63) + (1 << 39) + 1, 0x5f00_0001),
        (D::Bf16, (1_u64 << 31) + (1 << 23) + 1, 0x4f01),
    ] {
        let memory = Rc::new(FunctionalMemory::default());
        memory
            .write_values(0x1000, D::Uint64, &[V::from_u64(D::Uint64, input).unwrap()])
            .unwrap();
        let source = scalar(
            D::Uint64,
            output,
            1,
            &format!("{{ convert: [{READ}] }}"),
            false,
        );
        run(
            &platform("convert", &[D::Uint64], None),
            &source,
            Some(memory.clone()),
        )
        .unwrap();
        memory
            .check_values(
                0x3000,
                output,
                &[V::from_bits(output, expected_bits).unwrap()],
            )
            .unwrap();
    }
}

#[test]
fn nested_calls_round_to_the_assignment_dtype() {
    let expr = "{ sub: [{ add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }, { read: { tensor: A, indices: [i] } }] }";
    let platform = platform("add, sub", &[D::Bf16, D::Bf16], None)
        .replace("[[bf16, bf16]]", "[[bf16, bf16], [fp32, bf16]]");
    for (output, expected) in [(D::Bf16, 0.), (D::Fp32, 1.)] {
        let memory = Rc::new(FunctionalMemory::default());
        memory
            .write_values(0x1000, D::Bf16, &[V::from_f32(D::Bf16, 256.).unwrap()])
            .unwrap();
        memory
            .write_values(0x2000, D::Bf16, &[V::from_f32(D::Bf16, 1.).unwrap()])
            .unwrap();
        run(
            &platform,
            &scalar(D::Bf16, output, 1, expr, true),
            Some(memory.clone()),
        )
        .unwrap();
        memory
            .check_values(0x3000, output, &[V::from_f32(output, expected).unwrap()])
            .unwrap();
    }
}

#[test]
fn low_precision_matrix_operands_use_the_declared_fp32_accumulator() {
    for dtype in [D::Fp16, D::Bf16, D::Fp8, D::Fp4] {
        let memory = Rc::new(FunctionalMemory::default());
        let encode = |values: &[f32]| {
            values
                .iter()
                .map(|value| V::from_f32(dtype, *value).unwrap())
                .collect::<Vec<_>>()
        };
        memory
            .write_values(0x1000, dtype, &encode(&[1., 2., -3., 4., 0.5, -1.]))
            .unwrap();
        memory
            .write_values(0x2000, dtype, &encode(&[2., -1., 3., 4., 1., -2.]))
            .unwrap();
        let source = format!(
            r"
nodes:
  - {{ id: A, kind: tensor, config: {{ addr: 0x1000, dtype: {}, shape: [2, 3] }} }}
  - {{ id: B, kind: tensor, config: {{ addr: 0x2000, dtype: {}, shape: [3, 2] }} }}
  - id: gemm
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [m, n]
          expr: {{ matrix_mac: {{ over: [k], execution: {{ instruction: tensor, accumulator: fp32, sparsity: dense }}, args: [{{ read: {{ tensor: A, indices: [m, k] }} }}, {{ read: {{ tensor: B, indices: [k, n] }} }}] }} }}
    tiling: {{ sizes: {{ m: 1, n: 1, k: 1 }} }}
  - {{ id: Y, kind: tensor, config: {{ addr: 0x3000, dtype: fp32, shape: [2, 2] }} }}
edges:
  - {{ from: A, to: gemm.0, kind: data }}
  - {{ from: B, to: gemm.1, kind: data }}
  - {{ from: gemm.0, to: Y, kind: data }}
",
            name(dtype),
            name(dtype)
        );
        let platform = platform("matrix_mac", &[dtype, dtype], Some(D::Fp32));
        let functional = run(&platform, &source, Some(memory.clone())).unwrap();
        assert_eq!(memory.read_f32(0x3000, 4).unwrap(), [5., 13., 8.5, 0.]);
        assert_eq!(functional, run(&platform, &source, None).unwrap());
    }
}

#[test]
fn scaled_fp4_matrices_decode_real_metadata_with_partial_blocks_and_padding() {
    for format in ["nvfp4", "mxfp4"] {
        for layout in ["logical", "padded128x4"] {
            let nv = format == "nvfp4";
            let block: usize = if nv { 16 } else { 32 };
            let k = block + 1;
            let columns = if layout == "logical" { 2 } else { 4 };
            let rows = if layout == "logical" { 2 } else { 128 };
            let memory = Rc::new(FunctionalMemory::default());
            let a = (0..2 * k)
                .map(|i| [1., -0.5, 2., -1.][i % 4])
                .collect::<Vec<_>>();
            let b = (0..2 * k)
                .map(|i| [0.5, 2., -1., 1.5][i % 4])
                .collect::<Vec<_>>();
            for (address, values) in [(0x1000, &a), (0x2000, &b)] {
                let packed = values
                    .iter()
                    .map(|value| V::from_f32(D::Fp4, *value).unwrap())
                    .collect::<Vec<_>>();
                memory.write_values(address, D::Fp4, &packed).unwrap();
            }
            let scale_a = [[0.5, 2.], [1., 4.]];
            let scale_b = [[2., 0.25], [0.5, 1.]];
            load_scale_planes(&memory, scale_a, scale_b, (rows, columns), nv);
            let (inputs, metadata, globals, global_a, global_b, global_edges) = if nv {
                memory.write_f32(0x6000, &[2.]).unwrap();
                memory.write_f32(0x7000, &[3.]).unwrap();
                (
                    "A, B, SA, SB, GA, GB",
                    "SA, SB, GA, GB",
                    "  - { id: GA, kind: tensor, config: { addr: 0x6000, dtype: fp32, shape: [1] } }\n  - { id: GB, kind: tensor, config: { addr: 0x7000, dtype: fp32, shape: [1] } }",
                    ", global_scale: GA",
                    ", global_scale: GB",
                    "  - { from: GA, to: gemm.4, kind: data }\n  - { from: GB, to: gemm.5, kind: data }",
                )
            } else {
                ("A, B, SA, SB", "SA, SB", "", "", "", "")
            };
            let source = format!(
                r"
nodes:
  - {{ id: A, kind: tensor, config: {{ addr: 0x1000, dtype: fp4, shape: [2, {k}] }} }}
  - {{ id: B, kind: tensor, config: {{ addr: 0x2000, dtype: fp4, shape: [2, {k}] }} }}
  - {{ id: SA, kind: tensor, config: {{ addr: 0x4000, dtype: fp8, shape: [{rows}, {columns}] }} }}
  - {{ id: SB, kind: tensor, config: {{ addr: 0x5000, dtype: fp8, shape: [{rows}, {columns}] }} }}
{globals}
  - id: gemm
    kind: compute
    pe: pe0
    program:
      inputs: [{inputs}]
      metadata_inputs: [{metadata}]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [m, n]
          expr:
            matrix_mac:
              over: [k]
              execution: {{ instruction: {format}, accumulator: fp32, sparsity: dense }}
              args: [{{ read: {{ tensor: A, indices: [m, k] }} }}, {{ read: {{ tensor: B, indices: [n, k] }} }}]
              quantization:
                - {{ format: {format}, block_size: {block}, block_scales: SA{global_a}, layout: {layout} }}
                - {{ format: {format}, block_size: {block}, block_scales: SB{global_b}, layout: {layout} }}
    tiling: {{ sizes: {{ m: 1, n: 1, k: 7 }} }}
  - {{ id: Y, kind: tensor, config: {{ addr: 0x3000, dtype: fp32, shape: [2, 2] }} }}
edges:
  - {{ from: A, to: gemm.0, kind: data }}
  - {{ from: B, to: gemm.1, kind: data }}
  - {{ from: SA, to: gemm.2, kind: data }}
  - {{ from: SB, to: gemm.3, kind: data }}
{global_edges}
  - {{ from: gemm.0, to: Y, kind: data }}
"
            );
            let platform = platform("matrix_mac", &[D::Fp4, D::Fp4], Some(D::Fp32))
                .replace("instruction: tensor", &format!("instruction: {format}"));
            let functional = run(&platform, &source, Some(memory.clone())).unwrap();
            let mut expected = Vec::new();
            for m in 0..2 {
                for n in 0..2 {
                    let sum = (0..k)
                        .map(|i| {
                            a[m * k + i]
                                * scale_a[m][i / block]
                                * b[n * k + i]
                                * scale_b[n][i / block]
                        })
                        .sum::<f32>();
                    expected.push(if nv { sum * 6. } else { sum });
                }
            }
            assert_eq!(
                memory.read_f32(0x3000, 4).unwrap(),
                expected,
                "{format} {layout}"
            );
            assert_eq!(functional, run(&platform, &source, None).unwrap());
        }
    }
}

fn load_scale_planes(
    memory: &FunctionalMemory,
    scale_a: [[f32; 2]; 2],
    scale_b: [[f32; 2]; 2],
    (rows, columns): (usize, usize),
    nv: bool,
) {
    for (address, scales, mx_bits) in [
        (0x4000, scale_a, [[126, 128], [127, 129]]),
        (0x5000, scale_b, [[128, 125], [126, 127]]),
    ] {
        // Padding is initialized but numerically invalid; the LSU must load it
        // without using it as a logical scale.
        let mut values = vec![V::from_bits(D::Fp8, 255).unwrap(); rows * columns];
        for row in 0..2 {
            for column in 0..2 {
                values[row * columns + column] = if nv {
                    V::from_f32(D::Fp8, scales[row][column]).unwrap()
                } else {
                    V::from_bits(D::Fp8, mx_bits[row][column]).unwrap()
                };
            }
        }
        memory.write_values(address, D::Fp8, &values).unwrap();
    }
}
