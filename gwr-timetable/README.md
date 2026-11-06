<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-timetable

`gwr-timetable` describes a workload as a graph of tensors and computations. A
timetable file defines the workload. A separate platform file defines the
machine. See `gwr-platform/README.md` for its format.

The simulator models compute time, storage, and memory traffic. It does not
calculate tensor values.

## Run an example

### Add then multiply

`gwr-timetable/examples/add_mul.yaml` describes a simple graph with two compute
nodes. The first compute node reads tensors `A` and `B` from memory and adds
them. It writes the intermediate tensor `T` to memory. The second compute node
reads `T` and a third input, `C`. It multiplies them and writes the result, `Y`,
to memory.

Run this command from the repository root:

```sh
cargo run --release --bin gwr-timetable -- \
  --platform gwr-timetable/examples/compute_region_platform.yaml \
  --timetable gwr-timetable/examples/add_mul.yaml \
  --stdout --stdout-level info
```

### Fused computation

The example in `gwr-timetable/examples/fused_add_mul.yaml` combines both steps
in one compute node. Its intermediate stays in PE SRAM. This removes the
intermediate store, reload, and dependency between compute nodes.

To run this optimization, use
`--timetable gwr-timetable/examples/fused_add_mul.yaml` in the command above.

## File structure

A timetable is a YAML file with two top-level fields:

| Field   | Purpose                                                 |
| ------- | ------------------------------------------------------- |
| `nodes` | Tensor nodes and compute nodes. Each has a unique `id`. |
| `edges` | Data connections and control relationships.             |

The following timetable adds two vectors on `pe0`:

```yaml
nodes:
  - id: lhs
    kind: tensor
    config: { addr: 0x1000, dtype: fp32, shape: [8] }
  - id: rhs
    kind: tensor
    config: { addr: 0x2000, dtype: fp32, shape: [8] }
  - id: result
    kind: tensor
    config: { addr: 0x3000, dtype: fp32, shape: [8] }
  - id: add
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr:
            add:
              - { read: { tensor: A, indices: [i] } }
              - { read: { tensor: B, indices: [i] } }
    tiling: { sizes: { i: 4 } }
edges:
  - { from: lhs, to: add.0, kind: data }
  - { from: rhs, to: add.1, kind: data }
  - { from: add.0, to: result, kind: data }
```

This example uses the same platform as the run command above. The addresses
select storage in that platform's memory map.

## Tensor nodes

A tensor node describes storage in external memory. It can be a workload input,
an output, or an intermediate between compute nodes.

```yaml
- id: weights
  kind: tensor
  config:
    addr: 0x4000
    dtype: bf16
    shape: [16, 32]
```

| Field          | Meaning                                                       |
| -------------- | ------------------------------------------------------------- |
| `id`           | Unique graph node name.                                       |
| `kind`         | Must be `tensor`.                                             |
| `config.addr`  | Address of the tensor's first byte.                           |
| `config.dtype` | Storage datatype of each element.                             |
| `config.shape` | Dimensions in storage order. Each dimension must be positive. |

### Shapes and storage

Tensors use a contiguous row-major layout. The last dimension varies fastest.
The `weights` example contains 512 BF16 elements and occupies 1,024 bytes.

Use `shape: []` for a scalar. Use `shape: [8]` for a vector and `shape: [4, 8]`
for a matrix. A scalar contains one element.

The datatype determines each element's storage size. Sub-byte elements are
packed into bytes. The total storage size rounds up to a whole byte.

### Tensor readiness

A tensor with no producer is ready at the start of the simulation. A produced
tensor becomes ready after every compute node writing it finishes. Consumers
then become eligible to run.

This is a whole-tensor dependency. A consumer cannot start after only part of
its input tensor has been produced. This also applies when producer and consumer
use the same PE.

## Compute nodes

A compute node runs one inline tensor program on a Processing Element (PE).

| Field          | Meaning                                                   |
| -------------- | --------------------------------------------------------- |
| `id`           | Unique graph node name.                                   |
| `kind`         | Must be `compute`.                                        |
| `pe`           | Platform PE name. Required to execute the node.           |
| `program`      | Inputs, outputs, temporaries, and assignments.            |
| `tiling`       | Optional axis sizes for splitting the program into tiles. |
| `input_views`  | Optional views of connected input tensors.                |
| `output_views` | Optional views of connected output tensors.               |

### Program ports

Program input and output names are local to the compute node. They need not
match graph tensor IDs. Their declaration order defines the port numbers.

In the complete example, `inputs: [A, B]` creates input ports 0 and 1. The edges
bind `lhs` to `A` and `rhs` to `B`. `outputs: [Y]` creates output port 0, which
binds to `result`.

An input can establish a dependency without being read by the program. Such an
input causes no PE load or SRAM reservation.

### Tensor views

A view selects a rectangular region of a connected tensor. Its offsets and
dimensions are measured in elements. The view retains the parent tensor's
storage strides.

```yaml
input_views:
  - { offsets: [4], shape: [8] }
  - null
output_views:
  - null
```

This selects eight elements of input 0, starting at element four. Input 1 and
output 0 use their entire connected tensors. Program indices are relative to the
selected views.

Each supplied list must have one entry per program port, in declaration order. A
`null` entry selects the whole tensor. Omitting either list selects whole
tensors for that direction. For a program with two inputs, omit `input_views` or
use `input_views: [null, null]` to select whole tensors. `input_views: []`
supplies no entries, so it is invalid for that program. The same rule applies to
`output_views`.

## Edges

Each edge has `from`, `to`, and `kind` fields.

### Data edges

A data edge binds a tensor to a compute port. It also establishes a scheduling
dependency.

```yaml
edges:
  - { from: lhs, to: add.0, kind: data }
  - { from: rhs, to: add.1, kind: data }
  - { from: add.0, to: result, kind: data }
```

Append `.0`, `.1`, and so on to select a compute input or output port. If the
index is omitted, the edge uses the next free port. Data connections must pass
through tensor nodes. Compute nodes cannot exchange data directly.

### Control edges

A `kind: control` edge records a relationship for graph display. It does not
delay execution.

### Memory conflicts

Two compute nodes may access the same memory bytes. If either writes, the graph
must order their execution through data edges. For example,
`writer -> tensor -> reader` makes the reader wait for the writer to finish. The
dependency can also pass through other nodes. Two nodes that only read the
shared bytes need no ordering. A compute node cannot read and write overlapping
external storage itself. Validation also checks aliases between different tensor
nodes.

## Tensor programs

A program names its values and describes the assignments that produce them.
`read`, `reduce`, and `matrix_mac` have dedicated expression syntax. Other
expression keys name platform operations.

| Field         | Meaning                                          |
| ------------- | ------------------------------------------------ |
| `inputs`      | Names bound to external input ports.             |
| `outputs`     | Names bound to external output ports.            |
| `temporaries` | Optional local values, each with a `dtype`.      |
| `assignments` | Expressions that produce outputs or temporaries. |
| `extents`     | Optional explicit axis sizes.                    |

### Assignments and temporaries

Each assignment gives its output axes in dimension order. `output` names the
value it produces. A read can use a program input or a value from another
assignment.

This program computes `(A + B) * C`:

```yaml
program:
  inputs: [A, B, C]
  outputs: [Y]
  temporaries: { T: { dtype: fp32 } }
  assignments:
    - output: T
      axes: [i]
      expr:
        add:
          - { read: { tensor: A, indices: [i] } }
          - { read: { tensor: B, indices: [i] } }
    - output: Y
      axes: [i]
      expr:
        mul:
          - { read: { tensor: T, indices: [i] } }
          - { read: { tensor: C, indices: [i] } }
```

`T` stays in PE SRAM. It needs no graph tensor node or external memory address.
Input and output datatypes come from connected tensors. Temporary datatypes come
from their declarations.

### Reads and axes

A read has one index per tensor dimension:

```yaml
read: { tensor: A, indices: [m, k] }
```

An index can be an axis name or an affine expression. The literal `0` selects a
singleton dimension. Use `indices: []` to read a scalar. A size-one dimension
can broadcast across a larger axis.

Local reads must use the producing assignment's axes in the same order. Within
one assignment, each external input must use one consistent index mapping.
Different assignments can use different mappings.

### Axis extents

An extent is the number of positions an axis covers in the full program
invocation. A direct read usually establishes it. For example, shape `[8]` with
`indices: [i]` gives `i` an extent of 8. Non-singleton reads of the same axis
must agree on its size.

Use `extents` when direct reads do not establish the required sizes:

```yaml
extents: { o: 3, k: 2 }
```

Here, `o` covers three positions and `k` covers two. Extents must be positive.
An affine index does not establish axis extents. Its axes need explicit extents
or sizes inferred from other direct reads.

### Named operations

Use a lowercase operation name as an expression key:

```yaml
sin:
  - { read: { tensor: A, indices: [i] } }
```

The platform must declare the operation and a matching capability. Ordered
operand datatypes select the capability. Matrix operations also match the
accumulator datatype. See "Input datatype matching" in `gwr-platform/README.md`.

Calls can nest, including inside reduction bodies. Use a named temporary when
several assignments need the same intermediate. The simulator counts operations
but does not evaluate their numerical results. It does not validate every
operation's output datatype against its inputs.

### Matrix products and accumulators

Use `matrix_mac` for a matrix product:

```yaml
- output: Y
  axes: [m, n]
  expr:
    matrix_mac:
      accumulator: fp32
      over: [k]
      args:
        - { read: { tensor: A, indices: [m, k] } }
        - { read: { tensor: B, indices: [k, n] } }
```

The output has axes `m` and `n`. The expression sums products over `k`. Both
operands must be direct reads of the single contraction axis. Affine indices and
excluded bounds are not supported here.

Either operand may use transposed matrix indices: `A[k,m]` with stored shape
`[K,M]`, `B[n,k]` with stored shape `[N,K]`, or both together. For every
orientation, the model assumes the selected matrix execution unit consumes that
layout directly, with no implicit transpose, repacking work or temporary
conversion buffer. Memory requests still follow the declared views, so their
costs can depend on layout. See
[Matrix operand layouts](../gwr-platform/README.md#matrix-operand-layouts) for
the supported combinations and the execution unit assumption.

`accumulator` is required. It specifies the datatype retained across contraction
chunks. The platform must advertise the same accumulator and input datatype
combination. If the output datatype differs, each result incurs one final
`convert` operation. That conversion occurs after all contraction chunks finish.
The platform must provide its capability too.

A sum of scalar products uses scalar multiply and add capabilities. Its index
shape does not turn it into a matrix execution unit operation.

### Reductions and index outputs

A reduction combines values across the axes in `over`. Those axes do not appear
in the result shape. Supported operations are `sum`, `min`, and `max`.

```yaml
- output: row_sum
  axes: [r]
  expr:
    reduce:
      op: sum
      over: [k]
      value: { read: { tensor: A, indices: [r, k] } }
```

Body arithmetic is counted separately from the reduction work. Reductions cannot
nest inside other reductions. A `min` or `max` reduction can read an input or a
local temporary.

Use `indices_output: I` on a `min` or `max` assignment to request an index
result. It can accompany `output`, or replace it when only indices are needed.
The index result has the same shape as the value result. Its datatype comes from
its output tensor or temporary declaration. The simulator models its storage and
traffic without calculating the indices.

### Affine windows and bounds

An affine index is a sum of axis terms and a constant offset. For example, this
index means `2*o + k - 1`:

```yaml
affine: { terms: { o: 2, k: 1 }, offset: -1 }
```

Reads default to `bounds: require_in_range`. Use `bounds: exclude` inside a
reduction to skip out-of-range samples. For a sum, excluded samples contribute
no arithmetic or input traffic. This can model padded convolution windows.

This program fragment takes the maximum of stride-two windows:

```yaml
extents: { o: 3, k: 2 }
assignments:
  - output: Y
    indices_output: I
    axes: [o]
    expr:
      reduce:
        op: max
        over: [k]
        value:
          read:
            tensor: A
            indices: [{ affine: { terms: { o: 2, k: 1 }, offset: -1 } }]
            bounds: exclude
```

Declare `Y` and `I` as program outputs or temporaries. Their connected tensors
or declarations supply the result datatypes.

## Tiling and execution

### Tile sizes

An axis extent describes the full program invocation. `tiling.sizes` controls
how much of each axis is processed at once:

```yaml
tiling: { sizes: { m: 16, n: 16, k: 32 } }
```

Here, output tiles cover up to 16 positions on `m` and `n`. Each contraction
chunk covers up to 32 positions on `k`. Tails use the remaining positions. An
omitted axis size covers its full extent.

A tile size applies wherever its axis appears, including reduction chunks.
Assignments with different output axes can run in separate inferred phases.
Program tiling is separate from an execution unit's matrix service tile. See
"Matrix service tiles" in `gwr-platform/README.md`.

### Automatic assignment phases

Assignment dependencies and `tiling.sizes` determine execution order and storage
lifetimes. When a chunked axis occurs in some assignment outputs but not others,
the runner separates the assignments into phases. Common output axes form outer
loops. Ready assignments with matching remaining axes share a tile group. The
planner keeps producer/consumer chains together and places independent row
statistics before chunked pipelines where possible. Within a group, dependencies
control assignment readiness and independent execution unit work can overlap.
Assignments sharing an initial input mapping share its load.

For example, fused softmax assignments `M = max(A)`, `S = sum(exp(A - M))`, and
`Y = exp(A - M) / S` need only ordinary tiling:

```yaml
tiling: { sizes: { r: 1, k: 64 } }
```

For each row, the runner computes the maximum and sum once, retains both local
statistics in SRAM, then produces output chunks. A 1,025-element FP32 row reads
12,300 bytes and writes 4,100 bytes: three input passes and one output pass,
without external traffic for the statistics. Final chunks cover the tail.

A temporary belongs to the least enclosing scope of its producer and consumers.
A chunked producer followed by a row reduction materializes the complete local
row before that reduction begins. A producer and consumer with the same axes can
instead keep their temporary chunk local. A boundary output consumed later in
the same program is also retained locally without a boundary reread.

Completed phases release scratch storage for reuse. Locals remain live until
their last use. Preflight reserves the maximum safe working set before any
memory requests. If a materialized row exceeds SRAM, choose another program or
tile shape; the runner reports insufficient capacity without recomputing phases
or spilling local tensors to external memory.

### SRAM and partial results

Each tile reserves PE SRAM for its working set. This includes input buffers,
output buffers, temporaries, and reduction scratch. Each local slot's reserved
size rounds up to a multiple of `SRAM_SLOT_ALIGNMENT_BYTES` (default 8) bytes.
Memory traffic uses the actual tensor-view ranges.

Partial results across compute batches use a destination or scratch slot. A
single matrix batch can keep its accumulator inside the execution unit. An
output conversion may require separate accumulator storage.

Tiles can overlap while their combined SRAM reservations fit. All compute nodes
on one PE share that capacity.

### Assignment and execution unit scheduling

An assignment starts when the values it reads are ready. The simulator groups
independent operations into batches by execution unit. Each batch occupies one
execution unit until it finishes. Batches sharing that execution unit wait for
one another. Independent batches on different execution units can overlap.

A compute node finishes after every tile has completed and stored its outputs.
Only then can downstream compute nodes use the produced tensors.

### Streamed row reductions

A reduction streams its complete reduction extent in chunks set by
`tiling.sizes`. A later chunked output reuses the completed statistic. Inline
arithmetic in the reduction's `value` expression needs only intermediate chunk
storage. Naming that arithmetic as a local tensor materializes the full region
required by its later consumers, which can require more SRAM.

Large sparse affine windows also need bounded tile sizes. Their accessed
positions are enumerated within each tile.

## Validation

Building the graph checks IDs, edges, port bindings, and tensor views. It also
checks programs, memory conflicts, and cycles across data edges. Creating a
runnable timetable checks that named PEs exist in the platform. It also checks
every compute node against its PE capabilities, tile SRAM limits, and memory
mappings. These checks run automatically in `Timetable::new()`. They issue no
memory requests.

Raw program syntax and owned tensor bindings become an immutable
`PreparedProgram`. Hardware-independent SRAM and traffic analysis traverses its
lazy schedule using bounded memory; traversal time can still scale with the
number of invocations. Preparing that program against a PE checks actual compute
work and durations, touched addresses, and SRAM capacity. The resulting shared
`PreparedTask` belongs to that exact PE instance, including when another PE has
the same name. The timetable retains this handle for dispatch and accounting.
Custom dispatchers can supply raw `ProgramTaskConfig` tasks; the PE prepares
them before activation or any memory request.

## Generate a timetable

`gen-timetable` creates synthetic add, matrix-product, and max-reduction graphs.
It uses the supplied platform to select PEs and tile sizes.

```sh
cargo run --release --bin gen-timetable -- \
  --platform gwr-platform/examples/platform.yaml \
  --out /tmp/generated-timetable.yaml \
  --partition-size 16
```

### Generated operations

The generator has three built-in program patterns:

| Operation             | Selection weight      |
| --------------------- | --------------------- |
| Elementwise addition  | `--weight-add`        |
| Matrix multiplication | `--weight-gemm`       |
| Maximum reduction     | `--weight-reduce-max` |

The weights control how often each operation is selected. Each weight defaults
to 1. Set a weight to 0 to disable that operation. For example,
`--weight-add 1 --weight-gemm 0 --weight-reduce-max 0` generates only additions.

### Graph partitions

The generator splits operations along an outer output dimension. Each partition
becomes a compute node with matching tensor views. Partitions are assigned to
eligible PEs in round-robin order. Each node contains its own inline program.

By default, there is roughly one partition per eligible PE. `--partition-size N`
requests about `N` positions along the split axis per partition. The partition
count need not be a multiple of the PE count. `--max-compute-nodes` caps the
total number of partitions. That cap may make partitions larger than requested.

### Compute capabilities

An operation can run only on PEs with matching compute capabilities and input
datatypes. The generator skips operations that no PE supports for those
datatypes. Generation fails if none of the enabled operations can run.

### SRAM capacity

The generator reduces internal tile sizes to fit the assigned PE's SRAM. It
fails if even a one-element tile cannot fit. A graph partition defines the work
assigned to one compute node. Internal tiles split that work into smaller pieces
for execution on its PE.

## Tracing and statistics

### PE activity traces

To record PE activity in Perfetto, run:

```sh
cargo run --release --bin gwr-timetable -- \
  --platform gwr-timetable/examples/compute_region_platform.yaml \
  --timetable gwr-timetable/examples/fused_add_mul.yaml \
  --perfetto \
  --perfetto-filter-regex '^top::pe0($|::lane::)' \
  --perfetto-file pe-activity.pftrace
```

The filter includes `pe0` and its compute, compute-wait, and LSU activity lanes.
Change `pe0` to another PE name, or omit the filter to include all tracked
entities. `--perfetto` does not enable console or binary trace logging. The
runner still prints a short run summary.

### Planned and observed traffic

Use `--dump-stats` to report planned boundary traffic and observed transfers.
The planned counts include reloads across tiles and reduction chunks. Physical
LSU accesses can transfer more bytes than the logical tensor payload. See
`gwr-timetable/examples/tiny.yaml` for a larger graph with tensor views.
