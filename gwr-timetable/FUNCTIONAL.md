<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Functional execution

Build with the optional `functional` feature and supply a data fixture to load
typed inputs and compare simulated outputs with independent references:

```sh
cargo run -p gwr-timetable --bin gwr-timetable --features functional -- \
  --platform gwr-timetable/examples/functional/platform.yaml \
  --timetable gwr-timetable/examples/functional/gemm.yaml \
  --functional-data gwr-timetable/examples/functional/data.yaml
```

This example multiplies a 2×5 matrix by a 5×3 matrix with ragged output and
reduction tiles, then checks all six output elements exactly.

The YAML fixture has `inputs` and `checks` lists. Each entry specifies a
physical byte `address`, an optional `dtype` (default `fp32`), and exactly one
source: numerical inline `values`, exact inline integer `bits`, or a binary
`file`. File paths resolve relative to the fixture. Integer values preserve
signed and unsigned 64-bit precision; boolean values are `true`/`false`.
Floating numerical values are read as FP32, then rounded to the selected dtype.
Use `bits` for exact encodings, including NaN payloads and packed scale bytes.

Binary files contain little-endian storage bits. `bool` packs eight values per
byte and `int4`/`fp4` pack two, least-significant bits first, with no row
padding. Packed files require an explicit logical `count`; unused final-byte
bits remain uninitialized. Other files must contain a whole number of elements.
Every bit consumed by arithmetic must have been initialized.

Integer, boolean and `bits` checks compare exact encodings. Floating numerical
checks use `abs(actual - expected) <= atol + rtol * abs(expected)`; defaults are
`atol: 0.00001` and `rtol: 0.0001`. At least one nonempty check is required.
Invalid tolerances, nonfinite numerical checked values, uninitialized operands,
and numerical mismatches fail the command. For example:

```yaml
inputs:
  - {
      address: 0x1000,
      dtype: uint64,
      values: [9007199254740993, 18446744073709551615],
    }
  - { address: 0x2000, dtype: fp8, bits: [56, 64] } # E4M3FN: 1, 2
checks:
  - { address: 0x3000, dtype: uint64, values: [9007199254740993] }
```

Ordinary builds exclude numerical storage and payload code. Even with the
feature compiled, numerical execution requires explicit enablement through the
CLI fixture or `ProcessingElement::enable_functional`. All participating PEs
must share the same `Rc<FunctionalMemory>` when using the library API.

Functional storage lives with the other memory models and is available as
`gwr_models::memory::FunctionalMemory` (with the `functional` feature enabled).

All fifteen declared dtypes have typed storage, readback, explicit conversion
and numerical execution. `FunctionalValue` retains exact bits and offers
`from_bits`, signed/unsigned integer, FP32 and boolean constructors.
`FunctionalMemory::write_values`, `read_values`, and `check_values` operate on
those values; the existing bulk `write_f32`, `read_f32`, and tolerance-based
`check_f32` remain available. Exact checks include NaN payloads and signed zero.

| Storage dtype                             | Functional numerical encoding                                      |
| ----------------------------------------- | ------------------------------------------------------------------ |
| `fp32`                                    | IEEE binary32                                                      |
| `fp16`                                    | IEEE binary16                                                      |
| `bf16`                                    | BF16, with the binary32 exponent range                             |
| `fp8`                                     | **E4M3FN**, bias 7, signed zero, NaN, maximum finite magnitude 448 |
| `fp4`                                     | **E2M1**, signed zero, all finite, maximum magnitude 6             |
| `int64`, `int32`, `int16`, `int8`, `int4` | Two's complement, exact declared width                             |
| `uint64`, `uint32`, `uint16`, `uint8`     | Unsigned, exact declared width                                     |
| `bool`                                    | One packed bit                                                     |

FP8 is explicitly E4M3FN, rather than an unspecified family of encodings. The
named `decode_e4m3fn`/`encode_e4m3fn` implementation is separate from the E8M0
scale decoder, so another FP8 encoding can be introduced with an explicit new
dtype/format contract. FP16, BF16, E4M3FN and E2M1 use nearest rounding with
ties to even. E4M3FN/E2M1 saturate overflow and infinity to the largest finite
magnitude. E4M3FN supports NaN; E2M1 rejects NaN because it has no encoding.
FP16/BF16 preserve infinity and quiet NaN behavior.

Direct and affine reads, singleton broadcasts and excluded bounds are supported.
An excluded read omits the entire contribution to a reduction. The numerical
operation contracts are:

| Operation                                                              | Supported domains                                                              |
| ---------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| `add`, `sub`, `mul`, `div`, `maximum`, `minimum`, `relu`, `neg`, `abs` | Floating, signed integer, or unsigned integer; `bool` supports minimum/maximum |
| `sqrt`, `exp`, `log`                                                   | Floating                                                                       |
| `bitand`, `bitor`, `bitxor`, `bitnot`                                  | Same-dtype integer or boolean operands/results                                 |
| `shift_left`, `shift_right`                                            | Integer value/result of the same dtype; signed or unsigned integer shift count |
| `equal`, `not_equal`, `less`, `less_equal`, `greater`, `greater_equal` | Same numeric domain, or boolean; result must be `bool`                         |
| `select`                                                               | Boolean condition and branches matching the result dtype                       |
| `convert`, `reinterpret`                                               | Every dtype; reinterpret requires equal logical bit widths                     |
| Sum/min/max reduction                                                  | Numeric; boolean minimum/maximum                                               |
| Dense `matrix_mac`                                                     | Numeric operands and a compatible numeric accumulator/output domain            |

Floating arithmetic uses binary32 opmath before rounding each result. Floating
operands may mix precisions. Signed/unsigned/floating domain changes require an
explicit `convert`. Every nested scalar call rounds to the enclosing
assignment's declared dtype; a cast does not create a higher-precision scope for
its children. Use a typed temporary assignment to retain wider opmath. Integer
operations and casts wrap modulo the destination width; signed division
truncates toward zero and `MIN / -1` wraps. Division by zero, negative shifts,
and shifts at least as wide as the source value fail explicitly. Signed right
shift is arithmetic. Floating-to-integer conversion truncates and saturates (NaN
becomes zero); integer-to-floating conversion rounds the exact integer once,
without passing through an intermediate floating dtype.

Sum reductions accumulate in the assignment dtype in logical index order.
Min/max compares direct reads in their source dtype, then converts the selected
value once to the output dtype; scalar-expression values already carry the
assignment dtype. A wholly excluded sum is zero; a wholly excluded min/max is an
error. Boolean sums and matrix arithmetic are undefined and rejected before
traffic. Matrix MACs use `execution.accumulator`, defaulting to the assignment
dtype when omitted. Each scalar product and each separate addition round to that
accumulator; the final value converts once to the output dtype. Operand values
retain their precision until product rounding. Integer MACs retain exact width
and signedness, including 64-bit products and sums modulo the accumulator width.

Both declared packed matrix formats execute numerically: NVFP4 uses E2M1 values,
E4M3FN block scales every 16 values, and an FP32 global scale per operand. MXFP4
uses E2M1 values and unsigned E8M0 scales every 32 values, without a global
scale. E8M0 byte zero is 2^-127, byte 254 is 2^127 and byte 255 is NaN; it must
be loaded as raw scale bits rather than ordinary E4M3FN numerical values. Each
operand is decoded as `value * block_scale * global_scale` in FP32 before the
ordinary FP32 product/add accumulation. Logical and `padded128x4` row-major
scale planes, partial final blocks and scale tensor subviews follow the
validated metadata contract. Padding is loaded by the actual LSU but is not an
arithmetic operand; no hardware scale swizzle is simulated. Structured sparse
matrices, index outputs, block pipelines and local indexed-round metadata
policies remain unsupported in functional mode and fail before program traffic.
Unknown operations and undefined type signatures fail before traffic;
unsupported memory endpoints fail when a request reaches them.

Direct two-dimensional FP32 dense contractions pack completed LSU data into
contiguous buffers and use native Accelerate SGEMM on macOS, with a
cache-blocked portable CPU kernel on other platforms. Dense FP32 NCHW
convolution expressions have a faster implementation selected by their indices
and expression structure, with scalar fallback for broadcasting. Native kernels
may fuse or reorder FP32 accumulation; use reference tolerances rather than
bitwise scalar equivalence. These fast paths require FP32 accumulation and
preserve the existing FP32 path. They do not emulate the rounding of a
particular simulated hardware instruction.

All numerical paths consume data delivered by actual simulated LSU loads and
publish physical memory only through scheduled stores. Packet initialization
masks retain missing-data detection at individual bits. Separate output masks
preserve neighboring packed bits, including tiles that share a physical byte.
Paged host storage keeps the bulk FP32 byte path. The diagnostic image retains
reduction inputs until each assignment finishes; its host memory usage can
exceed the modeled tile SRAM allocation. Numerical execution does not add
modeled traffic, alter packet geometry, or replace engine service timing.

Application workspaces can validate complete model graphs against an independent
numerical reference. The reusable functional execution and CLI gates are:

```sh
cargo test -p gwr-timetable --features functional --test functional_execution --test functional_cli
```
