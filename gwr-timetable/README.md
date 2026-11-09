<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-timetable

`gwr-timetable` describes a workload as a graph of tensors and computations.
The timetable file supplies the workload; a separate `gwr-platform` file
supplies the PEs, memory, and compute engines that run it.

## Timetable files

A timetable is a YAML file with these top-level fields:

| Field | Purpose |
| --- | --- |
| `nodes` | Tensor storage and computations, each with a unique `id`. |
| `edges` | Connections between nodes. |
| `programs` | Optional named programs shared by compute nodes. |
| `includes` | Optional paths to shared catalogs of named programs. |

An include path is relative to the file containing it, and catalogs may
include other catalogs. The same catalog file may be imported through more
than one path; duplicate program names from different files or the timetable
itself are errors. `TimetableFile::from_file` resolves includes, while
`from_string` accepts only self-contained YAML. A shared catalog can contain
`operation_classes`, `compute_profiles`, and `programs` together: a platform
imports the compute definitions and a timetable imports the programs. The
[ONNX catalog](examples/onnx/catalog.yaml) is one example; its runnable
programs are referenced by files under `examples/onnx/implemented/`.

Regenerate the pinned ONNX inventory, shared catalog, platform fixture, and
fixed-shape cases natively from the repository root:

```sh
cargo run --release --bin gen-onnx-examples -- --out-dir gwr-timetable
```

The generator embeds the pinned 200-operator inventory and declarative
program/gap catalog, then builds and validates 78 runnable graphs in Rust.
It records the remaining 122 operators as gaps. This is coverage of selected
fixed-shape cases at the pinned schema revisions, not a numerical ONNX runtime
or complete support for every attribute and shape. Use another `--out-dir` to
export a standalone directory containing `examples/onnx` and `tests/fixtures`.

Tensor nodes declare an external memory `addr`, storage `dtype`, and `shape`.
They can be inputs, outputs, or intermediates. Compute nodes name a `pe` in the
platform and supply a `program`, either inline or by name. They also have
optional `input_views` and `output_views` lists, with one entry for each
program input and output in declaration order when supplied. Omitting either
list gives every port in that direction a `null` view. A `null` entry selects
the whole connected tensor; an entry with `offsets` and `shape` selects a view
of it. For example,
`{ offsets: [4], shape: [8] }` selects eight elements starting at index four
from a one-dimensional tensor.

Each edge has `from`, `to`, and `kind` fields. A `data` edge connects a tensor
node to a compute input or a compute output to a tensor node. Append `.0`,
`.1`, and so on to a compute node ID to identify its input or output position:

```yaml
edges:
  - { from: A, to: add.0, kind: data }
  - { from: B, to: add.1, kind: data }
  - { from: add.0, to: Y, kind: data }
```

Edge indices are optional. If they are omitted, then the next free input or
output on the node will be connected.
Node IDs may contain dots, but a final decimal suffix always denotes a port:
`layer.0.add.1` selects port 1 of `layer.0.add`. Avoid node IDs ending in a
decimal dot suffix because an edge cannot refer to that ID without interpreting
its suffix as a port.

Data edges
also establish dependencies: a compute node becomes ready once its input
tensors are ready. A tensor produced by a compute node becomes complete
after that node finishes. If multiple compute nodes write to a tensor then
all nodes need to finish in order for the tensor to be complete.
An input may be bound solely to establish a dependency; if the program never
reads it, the PE does not load it or reserve SRAM for it.
Data connections must pass through tensor nodes; compute nodes cannot send data directly to one another.

Use `kind: dependency` to require completion without binding a tensor port,
for example `{ from: selection, to: gather, kind: dependency }`. Dependency
edges can connect tasks or tensors and participate in readiness and cycle
validation. `kind: control` records a relationship for graph display only;
it does not delay execution. Overlapping memory writes, or overlapping reads
and writes, require an ordering path between their tasks. A task cannot read
and write overlapping external storage itself.

Building a timetable graph validates node IDs, edge positions, tensor views,
program bindings, memory access conflicts, and the absence of cycles across
data and dependency edges.
Creating a runnable timetable with a platform also checks that named PEs exist.
Library callers can then call `Timetable::preflight()` to check platform
capabilities, tile SRAM admission, and transfer memory mapping before issuing
requests. The ordinary runner validates task execution as it proceeds.

The [tiny timetable](examples/tiny.yaml) shows tensor nodes, compute nodes,
views, and data edges together. The [fused region](examples/fused_region.yaml)
shows a smaller single-compute node workload. The following command runs it with a
matching platform:

```sh
cargo run --release --bin gwr-timetable -- \
  --platform gwr-timetable/examples/compute_region_platform.yaml \
  --timetable gwr-timetable/examples/fused_region.yaml \
  --stdout --stdout-level info
```

The `gen-timetable` tool generates synthetic add, matrix product, and
max-reduction graphs for a platform. It divides each operation along an outer
output dimension into contiguous compute nodes with matching tensor views,
then assigns consecutive partitions to PEs with matching typed compute
capabilities in round-robin order. Unsupported templates are skipped; the
tool reports an error if no enabled template can run on the platform. By
default the outer extent is divided into roughly one partition per eligible PE. Use
`--partition-size N` to request partitions of about `N` output elements along
that axis; the number of partitions need not be a multiple of the PE count.
`--max-compute-nodes` caps the total number of generated partitions and may
make them larger than the requested size. Partitioned nodes share one named
program in the timetable YAML. The generator also chooses smaller internal
tile sizes when needed to fit each assigned PE's SRAM, and reports an error
if even a one-element tile cannot fit. A platform file may import compute
catalogs.

```sh
cargo run --release --bin gen-timetable -- \
  --platform gwr-platform/examples/platform.yaml \
  --out /tmp/generated-timetable.yaml \
  --partition-size 16
```

To inspect the names available to a trace filter, run the entity helper with
the same platform, timetable, and regex. It runs the workload, prints every
matched entity name and the match count. Add `--show-all` to include unmatched
names, explicitly labelled `NO MATCH`. If an alternative name caused a match,
the helper prints that alias too. Activity lane names appear when the workload
uses them:

```sh
cargo run --release --bin gwr-timetable-entities -- \
  --platform gwr-timetable/examples/compute_region_platform.yaml \
  --timetable gwr-timetable/examples/fused_region.yaml \
  --perfetto-filter-regex '^top::pe0($|::lane::)'
```

The helper accepts exactly one of `--regex`, `--filter-regex`,
`--perfetto-filter-regex`, `--binary-filter-regex`, or
`--stdout-filter-regex`, so a filter can be copied from another command.
Omit the regex to list all names; a regex with no matches exits with an error
and suggests `--show-all`.

To record only the PE activity lanes in a Perfetto trace, run:

```sh
cargo run --release --bin gwr-timetable -- \
  --platform gwr-timetable/examples/compute_region_platform.yaml \
  --timetable gwr-timetable/examples/fused_region.yaml \
  --perfetto \
  --perfetto-filter-regex '^top::pe0($|::lane::)' \
  --perfetto-file pe-activity.pftrace
```

The filter includes the `pe0` entity and its compute, compute-wait, and LSU
activity lanes. Change `pe0` to the PE name in your platform, or omit the filter
to include all tracked entities. `--perfetto` does not enable console or binary
trace logging; the runner still prints a short run summary.

## Compute

Each compute node selects a tensor `program`. The program names its external
input and outputs, optional SRAM-local temporaries, and assignments. Input
datatypes come from the connected tensors; a temporary declares its datatype.
The result and optional index output use the datatypes of their connected
tensors. MAC accumulator width is handled by the hardware model and is not a
program operand. An explicit `matrix_mac.execution` can select the instruction
family, accumulator dtype, and sparsity; the platform must advertise the same
mode. If its accumulator dtype differs from the output dtype, the simulator
counts a final `convert` operation for each result. Engine selection uses the
operation, ordered input datatypes, and execution mode, not the result datatype.
Validation of whether an operation's
result datatype is appropriate for its inputs is deferred.

Programs shared by compute nodes can be defined once at the top level and
referenced by name. The node keeps its own PE, tensor views, and tiling, so the
same program can run on different PEs and tensor regions:

```yaml
programs:
  vector_add:
    inputs: [A, B]
    outputs: [Y]
    assignments:
      - output: Y
        axes: [i]
        expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
nodes:
  - id: add_0
    kind: compute
    pe: pe0
    program: vector_add
    tiling: { sizes: { i: 128 } }
  # Other tensor and compute nodes may refer to vector_add as well.
edges: []
```

A named program can call other named programs. Each call binds every callee
input and output to a value in the caller. Values between calls are declared
as caller temporaries and stay in PE SRAM for the tile:

```yaml
programs:
  add:
    inputs: [A, B]
    outputs: [Y]
    assignments:
      - output: Y
        axes: [i]
        expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
  mul:
    inputs: [A, B]
    outputs: [Y]
    assignments:
      - output: Y
        axes: [i]
        expr: { mul: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
  add_then_mul:
    inputs: [A, B, C]
    outputs: [Y]
    temporaries: { T: { dtype: fp32 } }
    calls:
      - { program: add, inputs: { A: A, B: B }, outputs: { Y: T } }
      - { program: mul, inputs: { A: T, B: C }, outputs: { Y: Y } }
```

Calls can be nested; their own
temporaries are kept separate. An optional `axes` map on a call renames callee
axes in the caller, for example `axes: { i: channel }`. Unmapped axes keep
their names. Composition is expanded when the timetable loads, then the same
program validation, tile scheduling, and engine selection apply as for a
program written with assignments directly.

For a program used by only one node, its definition may be placed directly
under that node's `program` field:

```yaml
program:
  inputs: [A, B, C]
  outputs: [Y]
  temporaries: { T: { dtype: fp32 } }
  assignments:
    - output: T
      axes: [i]
      expr:
        {
          add:
            [
              { read: { tensor: A, indices: [i] } },
              { read: { tensor: B, indices: [i] } },
            ],
        }
    - output: Y
      axes: [i]
      expr:
        {
          mul:
            [
              { read: { tensor: T, indices: [i] } },
              { read: { tensor: C, indices: [i] } },
            ],
        }
tiling: { sizes: { i: 4 } }
```

### Axis extents

An axis extent is the number of positions an axis covers in one invocation of
a program. The optional `extents` map belongs inside the program, whether the
program is inline or named. For example, `extents: { o: 3, k: 2 }` gives the
output axis `o` three positions and the reduction axis `k` two positions.
Extents must be positive.

The simulator infers an axis extent when a read uses that axis directly as a
tensor index: a read of a view with shape `[8]` and `indices: [i]` gives `i`
an extent of 8. Reads of size-one dimensions can broadcast across a larger
extent. Other non-singleton reads using the same axis must agree on its size.
Assignment output axes determine the result shape from these extents;
reduction axes cover their extents but do not appear in the result shape.

An affine index does not establish the extents of its axes. Declare any axes
used only in affine indices explicitly. For example, a stride-two window can
use `extents: { o: 3, k: 2 }` with the affine index `2*o + k - 1`: `o` selects
three output positions and `k` selects two samples per window. With
`bounds: exclude` in a `min` or `max` reduction, samples outside the input are skipped.

Extents describe the full program invocation. The compute node's `tiling.sizes`
map specifies how many positions of each axis a tile covers; omitting an axis
size makes one tile cover its full extent.

Every tile keeps its local temporaries in PE SRAM, so an intermediate
assignment produces no external memory access. A tile can start when enough
PE SRAM is available for it. Tiles from the same or
different compute nodes may overlap while their combined reservations fit;
all compute nodes on one PE share that SRAM capacity.

Within a tile, an assignment starts when the values it reads are ready. The
simulator groups its compute operations into batches by engine. A batch holds
its engine until it finishes, so another batch needing that engine waits.
Independent batches on different engines can run at the same time; dependent
batches run in order. A downstream compute node becomes ready only after the
producer has finished every tile and written its output tensor to external
memory, even when both nodes use the same PE.

### Expressions and assignments

Each assignment lists its output `axes` in dimension order and supplies an
`expr`. `output: NAME` binds the computed value. A `min` or `max` reduction
may also bind `indices_output: NAME`, or bind only `indices_output` when its
value is not needed. At least one output must be named. A read refers to a program input or a value produced by
another assignment. The following expression forms are executable; `read`
stands for `{ read: { tensor: NAME, indices: [INDEX, ...] } }`:

| Expression | Shape | Meaning |
| --- | --- | --- |
| `read` | One indexed read | Read a tensor or local temporary. |
| Operation name | `{ add: [read, read] }` or `{ sin: [read] }` | Execute a lowercase operation declared in the platform's operation classes. Arguments may contain further operation calls; their operand datatypes select the engine rate. Use `{ op_name: { args: [read], attrs: { ... } } }` for operator attributes. |
| `matrix_mac` | `{ matrix_mac: { over: [k], args: [read, read] } }` | Execute a matrix MAC across one reduction axis. |
| `reduce` | `{ reduce: { op: sum, over: [AXIS, ...], value: read } }` | Combine a read or scalar operation expression over the named axes with `sum`, `min`, or `max`. Body arithmetic is counted separately from reduction work. |

`read`, `reduce`, and `matrix_mac` have dedicated syntax. Every other
expression key is interpreted as a lowercase operation name and must be
declared by the platform. A program cannot declare operations with those three
reserved names.

For example, this assignment computes a matrix product. The `m` and `n` axes
form the output shape; `k` is reduced away:

```yaml
- output: Y
  axes: [m, n]
  expr:
    matrix_mac:
      over: [k]
      args:
        - { read: { tensor: A, indices: [m, k] } }
        - { read: { tensor: B, indices: [k, n] } }
```

Each read has one index per tensor dimension. An index can be an axis name
such as `m`, the literal `0` for a singleton dimension, or an affine index.
For example, `{ affine: { terms: { o: 2, k: 1 }, offset: -1 } }` means
`2*o + k - 1`. Affine axes need known extents, supplied by other direct reads
or by the program's `extents` map. Reads default to
`bounds: require_in_range`; `bounds: exclude` skips out-of-range positions
inside a reduction. Sum reductions can use affine indices to model windowed
convolutions, including padding where the excluded samples contribute zero.
Reduction axes may be tiled
separately from output axes.

A `min` or `max` reduction can bind an `indices_output` to an external output
or local temporary. It has the same shape as the optional value result and
uses the output or temporary's declared dtype. The simulator accounts for
this result's storage and traffic but does not calculate tensor values or
indices. For a stride-two window whose first sample is at `2*o - 1`, the expression
can use an affine read and exclude positions outside the input:

```yaml
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

Named-operation calls can nest, including in a reduction body; reductions
cannot be nested. `matrix_mac` requires two direct reads that index its single
reduction axis, without affine indices or excluded bounds.
Use separate assignments and named local temporaries to reuse intermediate values.
Local reads must use the producer's output axes in the same order. Each
input must use one consistent index mapping within an assignment. Different
assignments may read it differently. A `min` or `max` reduction
can read a boundary input or a local temporary. Every explicitly
tiled output axis that is not also reduced must occur in each assignment's
output axes. When an axis is both an output and reduction axis, local row
statistics can be streamed and recomputed for output chunks. This can reread
inputs and repeat arithmetic; see the [ML catalog](../gwr-use-cases/examples/ml/README.md).
Reducing a materialized local value across a tiled local axis requires a
full-row plan or an inline scalar expression instead.

An explicit `matrix_mac` expression requires a matching capability. A sum of products
uses scalar multiply and add capabilities regardless of its index shape.
Partial results spanning batches stay in a destination or local scratch slot; a
single matrix batch can keep its accumulator inside the engine. Local slots
are 8-byte aligned, while memory traffic uses the actual tensor-view ranges.
`--dump-stats` reports planned boundary traffic, including reloads across
tiles, separately from the transfers observed during simulation. Sparse
affine window accesses are enumerated within each tile, so large dilated
windows should use tile sizes that bound the number of accesses per tile.

The paired [fused region](examples/fused_region.yaml) and
[materialized region](examples/materialized_region.yaml) perform the same add
then multiply. The materialized graph exposes the intermediate as a tensor,
adding a store, a reload, and a whole-tensor scheduling barrier. The
[platform](examples/compute_region_platform.yaml) supplies separate add and
multiply engines for a repeatable timing comparison.

## Buffered block pipelines

Run the [small pipeline](examples/compute_region_pipeline.yaml) with its
[matching platform](examples/compute_region_pipeline_platform.yaml) for a complete example.

A compute node can opt into a bounded load/compute/store pipeline:

```yaml
tiling: {sizes: {b: 1, m: 512, n: 512, k: 256}}
pipeline:
  mode: overlap
  input_buffers: 2
  output_buffers: 1
  prefetch_distance: 1
```

Explicit pipelines support static copies, scalar expressions, sum/min/max reductions,
multi-axis reductions, affine/excluded windows, packed operands, supported native
matrix calls and dependent multi-assignment programs. The existing qualified dense
GEMM compiler remains a fast path. Instruction, accumulator precision, input types
and conversions must match platform capabilities. Indexed metadata policies,
scaled matrix payloads, `local_round_policy`, and boundary/local payload services
reject explicit requests before execution.

A **tile** is the boundary-output region selected by the program's output-axis
tiling. An **input chunk** is the boundary operand data for a scheduled compute
step; reduction-axis tiling determines update sizes. These names apply to the
program bindings and do not require GEMM axes. The generic executor keeps a
separate **compute working arena** for operand copies, temporaries, retained
reduction state and result scratch. Input buffers hold incoming chunks; output
buffers hold all completed boundary outputs for one tile until real store ACKs.

The complete reserved window is `input_buffers * input_slot_bytes +
output_buffers * output_slot_bytes + compute_working_bytes`. Generic execution
explicitly copies incoming slots into the working arena and completed results
into output slots, so a stored result cannot alias a later compute result.
Packed generic operands currently use conservative padding envelopes to cover
fragmented nibble-aligned physical ranges; this can select smaller tiles than a
future compacting implementation. Alignment and every requested slot are charged.

`prefetch_distance` bounds chunks ahead of the consumer, including across tiles.
It must be nonnegative and less than `input_buffers`. Assignment traversal uses
the validated topological order; intermediate assignment completion gates
successors. The final assignment permits prefetch of the next independent tile's
boundary data. Serial mode waits for every compute step and tile store ACK.

Both buffer counts may be any positive integer. Every configured slot,
working arena and required scratch allocation must fit simultaneously in the
PE SRAM pool, even when there are fewer remaining chunks or output tiles
than slots. The entire window is reserved atomically. Insufficient physical
capacity rejects the request; temporarily occupied capacity causes a wait.
The simulator never reduces requested counts or spills implicitly.

The platform's `block_execution` configuration supplies PE-wide stage limits
and storage backing. More buffers create queue space; they do not create
additional engines. Reads and acknowledged writes traverse the real LSU,
cache, fabric and memory path. A whole-node consumer becomes eligible only
after every output store completes. Buffer storage stays owned through
outstanding requests, including when the requesting task is cancelled.

Use `mode: serial` for a matched baseline with identical geometry, full
buffer reservation, arithmetic and logical boundary accesses. Cache misses and
physical memory traffic may change with scheduling. Timing may differ
because `overlap` permits independent stages to run together. See the
[Izanagi experiment](../gwr-use-cases/examples/izanagi/pipeline/README.md)
for qualified compute assumptions and reproducible comparisons.

## Transfers and communication

A `kind: transfer` node copies tensor views with equal element counts and
datatypes through
its issuing PE's memory path. It binds exactly one input and output with data
edges and needs no program or arithmetic capability:

```yaml
- id: copy
  kind: transfer
  pe: pe0
  buffer_bytes: 4096
  input_views: [{ offsets: [0], shape: [128] }]
  output_views: [{ offsets: [256], shape: [128] }]
```

The default buffer is 4096 bytes. It bounds each read-then-write payload;
transfers use a separate serialized transfer resource and do not reserve the
compute tile SRAM arena. Source and destination must be
reachable through that PE's memory map. The copy counts actual movement,
including packed sub-byte storage, without adding compute operations.
Packed views must have matching physical byte counts, including alignment
effects. Transfers use the PE's DMA issuer when configured and its LSU otherwise.
