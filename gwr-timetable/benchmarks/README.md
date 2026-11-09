<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Functional GEMM performance

`functional_gemm.py` runs real GEMM tensor programs through GWR, with numerical
execution enabled and disabled. Every functional run checks every output against
an independently accumulated FP64 reference rounded to FP32 for the fixture. The
script also verifies that simulated completion time is identical across modes.

## Measurements

Measured on an Apple M4 Pro, macOS 26.3, on 3 November 2026. These are host
execution times, not predictions of accelerator performance. Matrices are
square, dense FP32 with varied signed values. The full-output cases use enough
modeled SRAM to avoid tiling and 64 KiB LSU requests. Functional runs include
input loading, simulation and reference checking; fixture generation is outside
the timed subprocess. Results use normal desktop conditions and warm file
caches.

Whole-process wall time, median of five runs:

| Matrix    | Feature-off build | Functional build, numerical mode off | Functional mode on |
| --------- | ----------------: | -----------------------------------: | -----------------: |
| 512×512   |           17.9 ms |                              19.6 ms |            22.1 ms |
| 1024×1024 |           18.2 ms |                              19.8 ms |            29.6 ms |
| 2048×2048 |           19.3 ms |                              20.5 ms |            65.0 ms |
| 4096×4096 |           28.5 ms |                              24.6 ms |           275.7 ms |

Engine execution alone, excluding CLI loading and final checks:

| Matrix    | Feature-off build | Functional mode on |
| --------- | ----------------: | -----------------: |
| 512×512   |             <1 ms |               1 ms |
| 1024×1024 |             <1 ms |               5 ms |
| 2048×2048 |              1 ms |              18 ms |
| 4096×4096 |              2 ms |             123 ms |

Engine output has 1 ms resolution; a printed `0.000s` is not zero work. Process
startup dominates small cases, so small differences between disabled build
variants are noise rather than evidence of a speedup. All sizes preserved
simulated time exactly and passed the existing `atol=2e-5`, `rtol=2e-4` checks;
the largest absolute error was approximately `1.62e-5`.

Before optimization, 512×512 functional GEMM took **19.58 seconds** end to end
(median of three runs), versus **22.1 ms** afterward: approximately **885×**
faster. The previous evaluator looked up tensor metadata, decoded bytes and
interpreted expressions for every multiply-add. The new path packs completed LSU
data once, calls native CPU BLAS, and publishes through the original local
value/store boundaries. Initialization tracking now operates on 64-byte bitmask
words rather than individual bytes. A retained release diagnostic measured warm
16 MiB reads/writes falling from roughly 7–8 ms each to 1.2–1.5 ms each.

Tiling is a separate cost. With 128×128 output tiles and reduction chunks of 256
(median of three runs):

| Matrix    | Feature-off build | Functional mode on |
| --------- | ----------------: | -----------------: |
| 1024×1024 |            160 ms |             232 ms |
| 2048×2048 |            1.77 s |             2.63 s |

These cases execute many more simulated transfers and tasks. The large
whole-output speedup does not imply the same speedup for heavily tiled
workloads.

## CPU and GPU choice

Separate PyTorch host measurements compared CPU BLAS with Metal/MPS, using seven
warmed repetitions, explicit GPU synchronization, and the same FP64 references.
PyTorch CPU was configured with ten intra-op threads; this is not a claim about
the native GWR BLAS thread count.

| Matrix    | PyTorch CPU kernel | GPU kernel | GPU including uploads and download |
| --------- | -----------------: | ---------: | ---------------------------------: |
| 512×512   |           0.103 ms |   0.415 ms |                            1.03 ms |
| 1024×1024 |           0.718 ms |    1.84 ms |                            2.54 ms |
| 2048×2048 |            5.67 ms |    2.79 ms |                            4.39 ms |
| 4096×4096 |            45.1 ms |    20.7 ms |                            25.5 ms |

GWR uses native Accelerate SGEMM on macOS and a portable cache-blocked CPU
kernel elsewhere. No Python or PyTorch dependency is added to GWR itself. CPU
BLAS wins for smaller matrices; at 4096 the host-only GPU advantage including
transfers is approximately 20 ms. Even assuming that entire gain transferred to
GWR, it would save about 7% of the measured whole-process time before additional
integration costs. A production GPU backend is therefore deferred. Many small
tiles also make launch overhead more significant.

Native matrix kernels can fuse/reorder FP32 accumulation. Reference comparisons
use tolerances, not bitwise equality with the scalar fallback. Numerical work
and the Accelerate framework link remain absent from feature-off builds.

## Reproduce

Use a Python environment containing NumPy and PyTorch. Preserve separate
executables for the two build configurations; use a fresh directory for copies:

```sh
gemm_bench_dir="$(mktemp -d /tmp/gwr-gemm.XXXXXX)"
cargo build --release -p gwr-timetable --bin gwr-timetable --no-default-features
cp target/release/gwr-timetable "$gemm_bench_dir/timing"
cargo build --release -p gwr-timetable --bin gwr-timetable --features functional
cp target/release/gwr-timetable "$gemm_bench_dir/functional"

python gwr-timetable/benchmarks/functional_gemm.py \
  --binary "$gemm_bench_dir/functional" --default-binary "$gemm_bench_dir/timing" \
  --out outputs/gemm-benchmark --sizes 128 512 1024 2048 4096 --repeats 5 --cpu-threads 10

python gwr-timetable/benchmarks/functional_gemm.py \
  --binary "$gemm_bench_dir/functional" --default-binary "$gemm_bench_dir/timing" \
  --out outputs/gemm-tiled --sizes 1024 2048 --tile-size 128 --repeats 3 --cpu-threads 10

python gwr-timetable/benchmarks/functional_gemm.py --host-only \
  --out outputs/gemm-host --sizes 128 512 1024 2048 4096 --repeats 7 --cpu-threads 10
```

GPU comparisons need host GPU access. When MPS is unavailable, the host report
explicitly records that and contains CPU measurements only. Raw reports retain
each sample, binary paths/SHA256 hashes, environment and numerical errors;
fixtures are kept beside them. Timeouts are recorded as timeouts, not timings.
The checked-in [measurement record](results-2026-11-03.json) preserves the
baseline, intermediate optimization, final, tiled and host-only raw samples.

Run the retained kernel, layout, memory and composition tests separately:

```sh
cargo test -p gwr-models --features functional --lib functional_matmul
cargo test -p gwr-models --features functional --test functional_memory
cargo test -p gwr-timetable --features functional --test functional_execution --test functional_cli
cargo test -p gwr-use-cases --features functional --test resnet50_functional
cargo test --release -p gwr-models --features functional --lib bulk_payload_throughput -- --ignored --nocapture
```
