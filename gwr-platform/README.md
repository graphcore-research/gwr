<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-platform

<!-- ANCHOR: overview -->

`gwr-platform` defines the machine that runs a workload. A platform contains
processing elements (PEs), memories, caches, fabrics, and their connections. The
workload belongs in a separate timetable file. See `gwr-timetable/README.md` for
that format.

The `gwr_platform` library loads and validates platform YAML. It builds the
model components and connects their ports. It models time and resource use
without calculating tensor values.

## File structure

A platform file describes named resources and how they connect:

| Field                 | Purpose                                                                |
| --------------------- | ---------------------------------------------------------------------- |
| `memory_maps`         | Memories each PE can address.                                          |
| `operation_classes`   | Groups of operations with the same number of inputs.                   |
| `processing_elements` | PEs, their memory maps, SRAM, LSU settings, and execution units.       |
| `memories`            | External storage, addresses, capacities, and service rates.            |
| `caches`              | Optional caches between PEs and memory.                                |
| `coherency_managers`  | Directories that coordinate coherent caches over backing memory maps.  |
| `fabrics`             | Optional interconnects between resources.                              |
| `connections`         | Pairs of endpoints to connect.                                         |
| `defaults`            | Optional place to define YAML anchors for PE and cache configurations. |

## Coherent Caches

A cache can select directory endpoints with `coherency_managers: [cm0, cm1]`.
Each entry in the top-level `coherency_managers` section names a backing
`memory_map` and a `config.line_size_bytes`. Set the optional positive
`config.operations_per_tick` to limit directory processing; omitting it leaves
processing unlimited. Incoming packets, queued request starts, and barrier
completions share that budget. For example:

```yaml
coherency_managers:
  - name: cm0
    memory_map: backing_memory
    config:
      line_size_bytes: 32
      operations_per_tick: 1
```

Connect the directory endpoint `coherency_manager.cm0` to a fabric port, and
connect the cache's memory side to the fabric. Device routes are derived from
these connections.

Every selected manager must have the same positive line size as its cache.
Memory ranges must align to that size, and each physical memory can belong to
only one manager. Matching sizes need not be powers of two. The selected
managers must cover the connected processing element's memory map.

Coherent caches retain `max_inflight_parents` and `max_inflight_fills` (both
default to 64). The child bound includes read fills and exclusive acquisitions;
large requests stream through it. Dirty writebacks and invalidation acks share
one ordered control path. Barriers drain prior work without flushing resident
dirty lines. Hierarchical coherence is unsupported and rejected in platform
configuration.

The coherent examples demonstrate one and two managers. `gen-fabric` accepts
`--use-coherency-managers` with L1 caches and no L2 cache.

## A Simple Platform

This example connects one PE to memory through an L1 cache. The PE has one
execution unit that supports addition. Its memory map makes `mem0` addressable.

```rust
# use gwr_engine::engine::Engine;
# use gwr_platform::Platform;
# fn main() {
# let mut engine = Engine::default();
# let clock = engine.default_clock();
# let _ = Platform::from_string(&engine, &clock, "
memory_maps:
  - name: pe_memory_map
    devices:
      - mem0

operation_classes:
  add:
    operations: [add]
    arity: 2

processing_elements:
  - name: pe0
    memory_map: pe_memory_map
    config:
      lsu_access_bytes: 32
      sram_bytes: 64KiB
      compute:
        execution_units:
          vector:
            capabilities:
              - { class: add, ops_per_tick: 32 }

caches:
  - name: l1_0
    config:
      bw_bytes_per_tick: 32
      line_size_bytes: 32
      delay_ticks: 4

memories:
  - name: mem0
    kind: ddr
    base_address: 0x1_0000_0000
    config:
      capacity_bytes: 1GiB
      delay_ticks: 40

connections:
  - connect:
      - pe.pe0
      - cache.l1_0.dev
  - connect:
      - cache.l1_0.mem
      - mem.mem0
# ").unwrap_or_else(|err| panic!("failed to validate: {err}"));
# }
```

The memory map and connections serve different purposes. The map defines which
memory addresses the PE can use. The connections define the route taken by
memory requests and responses.

## Load a platform in Rust

`Platform::from_file()` reads YAML, validates it, and builds the model. Use
`Platform::from_string()` to load YAML held in a string.

```rust,no_run
use std::path::Path;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_platform::Platform;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::default();
    let clock = engine.default_clock();

    let platform = Rc::new(Platform::from_file(
        &engine,
        &clock,
        Path::new("gwr-platform/examples/simple_pe_cache_mem.yaml"),
    )?);

    println!("Platform has {} processing elements", platform.num_pes());
    println!("{platform}");
    Ok(())
}
```

<!-- ANCHOR_END: overview -->

## Processing elements

Each PE names its memory map and declares its configuration:

```yaml
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      num_active_requests: 8
      lsu_access_bytes: 32
      overhead_size_bytes: 8
      sram_bytes: 64KiB
      compute:
        execution_units:
          vector:
            capabilities:
              - { class: add, ops_per_tick: 32 }
```

This fragment assumes `mm` and the `add` operation class are declared elsewhere
in the file.

### SRAM and memory requests

| Setting               | Meaning                                                   |
| --------------------- | --------------------------------------------------------- |
| `sram_bytes`          | Shared storage capacity for program tiles.                |
| `num_active_requests` | Shared limit on outstanding read and write requests.      |
| `lsu_access_bytes`    | Size of each aligned load/store unit (LSU) memory access. |
| `overhead_size_bytes` | Protocol overhead added to memory packets.                |
| `compute`             | Named execution units and their capabilities.             |

Loads and stores share one LSU request path and one pool of request slots. A
slot remains occupied until the response arrives. A partial access still
transfers a full aligned LSU block. Memory, cache, and fabric models determine
service time and backpressure.

Program tiles reserve SRAM while they execute. Tiles can overlap when their
combined reservations fit. SRAM capacity and execution unit availability are
separate constraints.

### Repeated configurations

Execution units are declared in the PE configuration. The optional `defaults`
section stores YAML anchors; it does not automatically apply or merge a PE or
cache configuration. Reference an anchor explicitly at each resource that needs
it:

```yaml
defaults:
  pe_config: &shared_pe
    lsu_access_bytes: 32
    sram_bytes: 64KiB
    compute:
      execution_units:
        vector:
          capabilities:
            - { class: add, input_dtypes: [[fp32, fp32]], ops_per_tick: 32 }
processing_elements:
  - { name: pe0, memory_map: mm, config: *shared_pe }
  - { name: pe1, memory_map: mm, config: *shared_pe }
```

This fragment assumes `mm` and the `add` operation class exist. Each alias
reuses the entire anchored mapping. Repeat a configuration with explicit changes
when PEs need different settings. See `examples/platform.yaml` for a complete
multi-PE platform using whole-configuration aliases.

Omitted fields in a PE's actual `config` receive built-in field defaults:
`num_active_requests: 8`, `lsu_access_bytes: 32`, `overhead_size_bytes: 8`, and
`sram_bytes: 1MiB`. These apply independently of `defaults`; `compute` must
always be supplied. `--explain` on `gwr-timetable` prints effective PE settings.

For matched runnable recipes, use
`gwr-timetable/examples/compute_region_platform.yaml` with `add_mul.yaml` or
`fused_add_mul.yaml` from the same directory. Use
`gwr-timetable/examples/destination_precision_platform.yaml` with
`destination_precision.yaml` for nested FP32 work and BF16 output conversion.
The [timetable run commands](../gwr-timetable/README.md#run-an-example) and
[Rust example](../gwr-timetable/README.md#load-prepare-and-run-in-rust) load
these pairs.

## Memories and address maps

A memory declares a physical address range and service settings:

```yaml
memories:
  - name: mem0
    kind: hbm
    base_address: 0x1000_0000
    config:
      capacity_bytes: 1GiB
      bw_bytes_per_tick: 32
      delay_ticks: 40
memory_maps:
  - name: mm
    devices: [mem0]
```

`kind` selects the memory model, such as `hbm` or `ddr`. `capacity_bytes`
defines the range starting at `base_address`. `bw_bytes_per_tick` sets the
memory service rate. `delay_ticks` sets its delay. Use distinct address ranges
for distinct physical memories.

### Address and capacity units

Memory addresses and capacities accept integers, hexadecimal values, and size
strings. Size strings include `64KiB` and `1GiB`. The PE's `sram_bytes` accepts
these size strings too.

### Memory alignment

Each memory base address and capacity must be aligned to every effective LSU
access size. They must also be aligned to every configured cache line size.
Validation checks all physical memories, including memories absent from PE maps.

For power-of-two sizes, alignment to the largest size satisfies every
requirement. Other sizes must satisfy each alignment separately. These rules
keep LSU accesses and cache fills inside memory boundaries.

## Connections

Each `connect` entry names two endpoints:

```yaml
connections:
  - { connect: [pe.pe0, cache.l1_0.dev] }
  - { connect: [cache.l1_0.mem, mem.mem0] }
```

Each connection carries requests in one direction and responses in the other.
Cache `.dev` is the side facing a PE or other requester. Cache `.mem` is the
side facing downstream memory.

### Endpoint names

| Form                     | Endpoint                                |
| ------------------------ | --------------------------------------- |
| `pe.pe0`                 | PE named `pe0`.                         |
| `mem.mem0`               | Memory named `mem0`.                    |
| `cache.l1_0.dev`         | Requester-facing cache port.            |
| `cache.l1_0.mem`         | Memory-facing cache port.               |
| `fabric.fabric0@(1,2)`   | Port 0 at column 1, row 2 of `fabric0`. |
| `fabric.fabric0@(1,2).1` | Port 1 at that fabric node.             |

### Unused ports

Connect an unused fabric or cache endpoint to the string `null`:

```yaml
connections:
  - { connect: ["null", "fabric.fabric0@(11,16)"] }
```

This leaves that endpoint idle without an unconnected-port error. PE and memory
endpoints cannot connect to `null`. Quote `'null'` so YAML treats it as a
string. See `gwr-platform/examples/platform_null_connection.yaml` for a complete
example.

<!-- ANCHOR: compute_execution_units -->

## Execution units and capabilities

### Operation classes

A class groups named operations with the same number of inputs. Declare classes
at the platform level:

```yaml
operation_classes:
  arithmetic: { operations: [add, mul], arity: 2 }
  trig: { operations: [sin, cos, tan], arity: 1 }
  matrix: { operations: [matrix_mac], arity: 2, flops_per_op: 2 }
```

`arity` is the number of inputs per operation. Operation names must be lowercase
in both platform declarations and program calls. `flops_per_op` weights
operation counts for FLOP statistics. It defaults to 1 and does not change the
service rate.

### Execution unit declarations

Each named execution unit is an independent PE hardware resource. The simulation
`Engine` advances simulated time and runs these resources. Its capabilities
describe the operations and datatypes it can execute. Capabilities on the same
execution unit share its throughput and scheduling slot.

```yaml
compute:
  execution_units:
    vector:
      capabilities:
        - { class: arithmetic, input_dtypes: [[fp16, fp16]], ops_per_tick: 32 }
        - { class: trig, ops_per_tick: 32 }
    general:
      capabilities:
        - { class: arithmetic, ops_per_tick: 8 }
    matrix:
      capabilities:
        - { class: matrix, accumulator: fp32, ops_per_tick: 128 }
```

Place `compute` inside a PE's `config`. Every PE needs execution unit
declarations. No capability is assumed when it is absent.

| Capability field      | Meaning                                                                        |
| --------------------- | ------------------------------------------------------------------------------ |
| `class`               | Platform operation class supported by this execution unit.                     |
| `input_dtypes`        | Optional list of ordered input datatype combinations.                          |
| `ops_per_tick`        | Operations per tick for the whole execution unit. Must be positive and finite. |
| `accumulator`         | Required accumulator datatype for matrix capabilities.                         |
| `matrix_service_tile` | Optional block shape used to calculate matrix service time.                    |

### Input datatype matching

Each `input_dtypes` entry must contain `arity` datatypes, in operand order. For
example, `[fp16, bf16]` differs from `[bf16, fp16]`. Omitting `input_dtypes`
supplies a default for combinations without an explicit declaration.

For each operation, matching considers all execution units on the PE. If any
capability explicitly lists the requested inputs, only those matches are
eligible. Otherwise, capabilities without `input_dtypes` are eligible. Matrix
capabilities must also match the requested accumulator datatype. This
accumulator check applies before choosing between explicit and default
declarations.

In the example above, FP16 addition uses `vector` at 32 operations per tick. It
waits if `vector` is busy; it cannot use `general` instead. FP32 addition uses
`general` at 8 operations per tick. No capability explicitly lists FP32
addition.

### Program calls and operation counts

A timetable program calls an operation by name:

```yaml
sin:
  - { read: { tensor: A, indices: [i] } }
```

Operand datatypes come from the program's tensors and intermediate expressions.
Output datatypes come from its tensor and temporary declarations. Every scalar
call result, including nested calls, uses its assignment destination dtype;
reads retain their declared dtype. FP32 leaves do not promote intermediate
results. Implicit scalar result transitions add no `convert` work. Output
datatypes do not select execution units. For FP32 arithmetic before a BF16
store, use a FP32 temporary and a separate explicit `convert` assignment;
wrapping a nested expression in `convert` does not create a child dtype scope.
See
[Destination precision](../gwr-timetable/README.md#destination-precision-and-nested-calls)
and the
[operation cost table](../gwr-timetable/README.md#modeled-operation-costs). The
simulator does not check every operation's output datatype against its inputs.

Statistics report named operation counts and weighted FLOP equivalents. For
example, 16 matrix MACs with `flops_per_op: 2` contribute 32 FLOPs. The model
does not calculate numerical results.

### Batch scheduling

A compute batch groups work to execute on one execution unit. It holds that
execution unit until it finishes. Other batches needing the same execution unit
wait. Independent batches on different execution units can overlap. When several
execution units match, an available eligible execution unit can run the batch.

A program can produce several batches as it executes. Its data dependencies
determine which work can overlap. Fused temporaries stay in SRAM. A downstream
compute node waits until its producer has stored all output tiles.

### Service time

A compute batch takes `ceil(sum(count / ops_per_tick))` ticks, excluding queue
time. The sum includes each operation's count and matching capability rate. For
example, 32 `sin` operations and 32 `cos` operations share the `vector`
execution unit. At 32 operations per tick, that batch takes two ticks. Matrix
service tiles can increase the count used for timing.

### Matrix accumulators

Matrix capabilities declare their accumulator datatype:

```yaml
- class: matrix
  input_dtypes: [[bf16, bf16]]
  accumulator: fp32
  ops_per_tick: 128
```

The program requests the same datatype inside `matrix_mac`:

```yaml
matrix_mac:
  accumulator: fp32
  over: [k]
  args:
    - { read: { tensor: A, indices: [m, k] } }
    - { read: { tensor: B, indices: [k, n] } }
```

Both declarations require `accumulator`. Other operations omit it. The datatype
specifies the precision retained across contraction chunks. It also affects the
storage required for partial results.

If the output datatype differs, each result incurs one final `convert`
operation. The conversion runs after all contraction chunks finish. The platform
must declare the conversion capability too.

### Matrix operand layouts

Transposed operands are common in ML workloads:
[linear layers](https://docs.pytorch.org/docs/stable/generated/torch.nn.Linear.html)
compute `X * Wᵀ + bias`, and
[scaled dot-product attention](https://docs.pytorch.org/docs/stable/generated/torch.nn.functional.scaled_dot_product_attention.html)
forms `Q * Kᵀ`.

For `Y[m,n] = sum_k A * B`, `matrix_mac` supports all four operand orientations:

| Transposed operands | First operand: stored shape and read | Second operand: stored shape and read |
| ------------------- | ------------------------------------ | ------------------------------------- |
| Neither             | `[M, K]`, `A[m,k]`                   | `[K, N]`, `B[k,n]`                    |
| First only          | `[K, M]`, `A[k,m]`                   | `[K, N]`, `B[k,n]`                    |
| Second only         | `[M, K]`, `A[m,k]`                   | `[N, K]`, `B[n,k]`                    |
| Both                | `[K, M]`, `A[k,m]`                   | `[N, K]`, `B[n,k]`                    |

The same per-operand rule applies within each matrix batch. The contraction axis
remains `k` and the output remains `[M, N]` in every case.

**Execution unit assumption:** every eligible matrix execution unit can consume
either operand, or both operands together, in these transposed orientations at
its declared MAC rate. This applies with and without `matrix_service_tile`. No
implicit transpose operation, data copy, repacking buffer, additional compute
work, or temporary SRAM reservation is inserted or charged for changing the
orientation. Logical `m`, `n` and `k`, accumulator demand and service-tile
padding are independent of the operands' stored axis order.

Memory accesses still follow the declared tensor shapes, indices and views.
Transposed layouts can change access ranges, packet counts, cache behaviour and
memory timing, especially when the program is tiled. The assumption removes an
additional layout-conversion cost; it does not guarantee identical memory costs
for every layout. A platform requiring a separate transpose or repacking step
must model its cost explicitly; execution unit capabilities do not currently
describe operand-layout restrictions.

### Matrix service tiles

`matrix_service_tile` models an execution unit that processes matrix work in
fixed-size blocks. Declare it on a matrix capability:

```yaml
- class: matrix
  input_dtypes: [[bf16, bf16]]
  accumulator: fp32
  ops_per_tick: 128
  matrix_service_tile: { m: 16, n: 16, k: 16 }
```

Multiplying an `m × k` matrix by a `k × n` matrix produces an `m × n` result.
`k` is the contraction dimension. For each program tile and contraction chunk,
each dimension rounds up to a service-tile multiple. The product of the rounded
dimensions determines the MAC count used for timing for each matrix. Independent
matrices in a batch are charged separately; the batch count is not padded.

A `2 × 5` by `5 × 3` multiplication has 30 logical MACs. The declaration above
charges a `16 × 16 × 16` block, or 4,096 MACs. At 128 MACs per tick, it takes 32
ticks on an idle execution unit. Without the service tile, it takes
`ceil(30 / 128) = 1` tick.

Only compute time uses the rounded dimensions. Traffic, SRAM, accumulator
demand, and reported operation counts use actual dimensions. The program's
`tiling` setting separately controls how its work is split.

All three service-tile dimensions must be positive. The capability must
explicitly list input datatypes. Its class must contain only the two-input
`matrix_mac` operation. Every matrix operation carries logical dimensions, even
when no service tile is configured. The runner derives them from direct,
in-range operand indices and the current program tile:

- Output axes used only by the first operand form `m`.
- Output axes used only by the second operand form `n`.
- Output axes shared by both operands, or absent from both, form independent
  batches.
- The single contraction axis supplies `k`.

Multiple axes in one role multiply into that role's extent. Singleton operand
dimensions broadcast over their named logical axes. Batched matrices, broadcast
operands and transposed indices use the same geometry path. Logical MACs are
`batches * m * n * k`; a separate MAC count cannot replace these dimensions.
Without a service tile, timing uses this logical count. With a service tile, it
uses `batches * rounded_m * rounded_n * rounded_k`.

### Matrix accumulator capacity

An execution unit can optionally limit its accumulator storage:

```yaml
matrix:
  matrix_accumulator_capacity_bytes: 4096
  capabilities:
    - class: matrix
      input_dtypes: [[bf16, bf16]]
      accumulator: fp32
      ops_per_tick: 128
```

The limit applies to the logical output tile, independently of PE SRAM. An
`m × n` tile with an FP32 accumulator needs `m * n * 4` bytes. Service-tile
rounding does not increase this demand. Batched operations multiply the demand
by the batch count. An execution unit cannot run a batch whose demand exceeds
its limit. Omitting the setting leaves accumulator capacity unconstrained.

The capacity must be positive. Matrix capabilities on that execution unit must
explicitly list input datatypes.

<!-- ANCHOR_END: compute_execution_units -->

## Caches

This section describes non-coherent caches. See
[Coherent Caches](#coherent-caches) for coherent-cache configuration and
behavior.

Caches sit between requesters and downstream memory. They model line fills,
replacement, service bandwidth, and finite request capacity. They do not model
numerical data.

```yaml
caches:
  - name: l1_0
    config:
      line_size_bytes: 32
      bw_bytes_per_tick: 32
      num_ways: 4
      num_sets: 128
      delay_ticks: 4
      max_inflight_parents: 64
      max_inflight_fills: 64
```

### Lines and writes

A read miss fetches a whole aligned cache line. A request crossing line
boundaries waits for every required fill. Concurrent reads of the same missing
line can share a fill. A `NoAllocate` read miss fetches whole lines without
retaining them in the cache.

Writes keep their original byte traffic and invalidate overlapping lines.
Replacement and invalidation prevent stale responses from installing old
contents. Every memory range must meet the line-alignment rules described above.

### Request and fill limits

`max_inflight_parents` limits tracked original client reads.
`max_inflight_fills` limits downstream line reads awaiting responses. Both
default to 64. Large reads issue their fills as capacity becomes available.

A read completes after all its fills and return-data service finish. Original
requests and downstream fills each incur their own protocol headers. Hits and
completed misses share the bandwidth used to return data to clients.

### Cache statistics

`Cache::traffic_snapshot()` reports client payload, downstream payload, and
headers. It also reports hits, misses, and shared fills.
`inflight_fills_snapshot()` reports current and peak fill occupancy. These are
analytical model statistics, not hardware calibration results.

## Fabrics

A fabric connects resources through a grid of nodes. Its declaration sets the
grid size and model kind.

```yaml
fabrics:
  - name: fabric0
    kind: routed
    columns: 2
    rows: 2
    config:
      routing: column-first
      ticks_per_hop: 1
      port_bits_per_tick: 65536
      internal_link_bits_per_tick: 256
```

A `functional` fabric estimates transit delay from the horizontal and vertical
hop count. A `routed` fabric models traffic through individual routers and
links. Only routed fabrics support internal link bandwidth limits. Fabric
endpoints use column and row coordinates, as shown in the connections section.

### External ports and internal links

`port_bits_per_tick` sets the external port service rate.
`internal_link_bits_per_tick` optionally limits each directed node-to-node link.
Traffic sharing one directed link competes for its bandwidth. Opposite
directions and disjoint links can overlap.

The internal rate must be positive and is supported only by routed fabrics.
Omitting it leaves node-to-node timing governed by hop delay without an internal
bandwidth limiter. It does not change routes, external port rates, buffers, or
hop delays. YAML output preserves an explicit rate and omits an absent one.

### Internal link timing

The limiter charges `ceil(total_packet_bytes * 8 / internal_link_bits_per_tick)`
ticks. The packet size includes protocol overhead. It forwards the packet before
waiting for that interval. It then delays the upstream acknowledgement after
downstream acceptance.

The first packet can arrive before the interval ends. Later traffic on the same
link is spaced by the interval. This models throughput rather than physical
serialization latency.

### Buffers and backpressure

A downstream stall eventually fills the packet pipeline and byte buffers. Once
these fill, upstream senders must wait.

`ticks_per_hop` must be positive. Each hop can queue up to `ticks_per_hop`
packets. It can also hold one packet while forwarding. Routers and arbiters can
also hold packets while forwarding. These packet-count limits are separate from
`rx_buffer_bytes` and `tx_buffer_bytes`.

## Generate a fabric platform

`gen-fabric` creates a platform around a two-dimensional fabric. For example:

```sh
cargo run --release --bin gen-fabric -- \
  --out /tmp/platform.yaml \
  --pe-adds-per-tick 32 \
  --pe-muls-per-tick 8 \
  --pe-compares-per-tick 32 \
  --pe-matrix-macs-per-tick 128
```

The first three rates configure the shared ALU execution unit. The matrix rate
configures a separate execution unit with an FP32 accumulator. Each matrix MAC
counts as two FLOPs. The generator omits the internal link bandwidth limit by
default.

## Choose model parameters

Start with physical storage and address ranges. Assign memories to PE maps, then
connect local and remote paths. Next, declare compute capabilities, SRAM
capacity, and LSU access sizes. Set memory, cache, and link rates and delays.

Rates are measured per tick of the component's clock. State the clock period
when comparing results in units per second. Choose service tiles and capacity
limits to represent the machine being studied.
