<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-platform

<!-- ANCHOR: overview -->

`gwr_platform` is the library used to define and build execution platforms from
configuration files.

A platform in GWR is the structural description of the machine that a workload
runs on. It brings together named processing elements, memories, caches,
fabrics, and the connections between them into one validated object.

## What It Provides

The `gwr_platform` library provides:

- YAML configuration file support.
- build functions that construct memories, processing elements, caches, and
  fabrics via the `gwr_platform::builder` module.
- connection functions that wire a platform together via the
  `gwr_platform::connect` module.

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

An example of a simple platform could include:

- a processing element
- an L1 cache
- a backing memory
- a memory map describing what memory the PE can address

For example:

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
        engines:
          vector:
            capabilities:
              - { class: add, ops_per_tick: 256 }

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

Each memory in a processing element's memory map must have a base address and
capacity that are multiples of that PE's `lsu_access_bytes` (32 by default).
This keeps each aligned LSU access entirely within one mapped memory region.
When PEs share a memory map, its memories must satisfy every PE's access size.

Connect an unused fabric endpoint to `null` so its ingress and egress ports are
deliberately left idle without causing an unconnected-port error when the
simulator starts. For example: `connect: ['null', "fabric.fabric0@(11,16)"]`.
Cache device and memory ports can also connect to `null`; processing elements
and memories cannot. Quote `null` in YAML so it is parsed as a string. See
`gwr-platform/examples/platform_null_connection.yaml` for a complete
configuration.

## Example

Load a platform from YAML and inspect the resulting structure:

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

## Internal routed-link bandwidth

A routed fabric may declare `internal_link_bits_per_tick` independently of its
external `port_bits_per_tick`. This will give every node-to-node connection an
independent limiter in each direction. Streams sharing one directed link compete
for that bandwidth; opposite directions and disjoint links can overlap. Omitting
the setting provides only a node-to-node hop delay.

```yaml
config:
  port_bits_per_tick: 65536
  internal_link_bits_per_tick: 256
```

The rate must be positive and functional fabrics reject this setting. It does
not change routing, external port rates, buffers, or hop delays. YAML output
preserves explicit values and omits absent ones; `gen-fabric` continues to omit
the setting by default.

The limiter charges `ceil(total_packet_bytes * 8 / bits_per_tick)` ticks,
including the packet type's declared overhead. It forwards before waiting for
this rate limit interval and delays the upstream acknowledgement after
downstream acceptance. The first packet can therefore arrive before the interval
ends; subsequent traffic on the same link is spaced by that interval. This
models throughput rather than waiting for physical serialization before
delivery.

Downstream stalls propagate back-pressure once the finite packet pipeline and
configured ingress/egress byte buffers fill. Each positive hop delay allows at
most `ticks_per_hop` pending packets plus one packet held by its output task.
`ticks_per_hop` must be positive; zero-tick hops are rejected. Routers and
arbiters also hold a finite number of packets while forwarding. These are
packet-count pipeline limits, separate from the configured byte-buffer
capacities.

## Cache ranges and fill admission

Cache read misses fetch entire aligned `line_size_bytes` lines and return the
original request's payload. Requests spanning multiple lines wait for every
required fill. Writes retain their original byte traffic and invalidate every
overlapping line. Overlapping allocating reads share pending line fills;
`NoAllocate` misses fetch full lines without populating the cache. Replacement
and invalidation prevent old responses from marking new cache contents valid.

Every physical memory's base address and capacity must be multiples of every
configured cache line size and effective LSU access size. For power-of-two
sizes, alignment to the largest size satisfies this rule. Other sizes must
satisfy each alignment separately. Validation applies to all memories, including
ones absent from PE memory maps, and rejects invalid boundaries before building
the simulator.

`max_inflight_parents` bounds tracked original client reads (parents) and
`max_inflight_fills` bounds downstream fills; both default to 64. Large reads
stream through these bounds. Completion waits for every required fill and its
full response ingress service. Original client headers and each downstream fill
header are charged at their respective request service points. Hits and
completed misses share the client return-service bandwidth.

`Cache::traffic_snapshot()` separates logical client payload, rounded downstream
payload, headers, hits, misses and coalescing. `inflight_fills_snapshot()`
reports current and peak physical fill occupancy. The cache remains an analytic
round-robin model without coherence or numerical data evaluation; these settings
do not establish hardware calibration.

<!-- ANCHOR: compute_engines -->

## Operation classes and compute engines

Capabilities can live in a catalog shared by multiple platform files. A
catalog contains `operation_classes` and `compute_profiles`; each profile is
the complete engine configuration for one PE. For example,
`examples/default_compute.yaml` defines `basic_alu` with 16 adds, 4
multiplies, and 16 compares per tick on its ALU, plus a separate engine for
32 matrix MACs per tick. Its `wide_alu` profile has a separate matrix engine
for 256 MACs per tick:

```yaml
includes: [default_compute.yaml]
processing_elements:
  - name: pe0
    memory_map: mm
    config: { compute: basic_alu }
```

This is a catalog/PE configuration fragment. A runnable platform also needs
the `mm` memory map, memories, and connections, as in the example above.

`includes` paths are relative to the file that declares them. Catalogs can
include other catalogs. Re-importing the same file is harmless; duplicate
definition names, include cycles, missing files, and unknown profile names are
errors. A platform may also define its own classes and profiles alongside its
topology, or put an inline `engines` map in a PE's `compute` field. Use
`Platform::from_file` for includes; `from_string` accepts only self-contained
YAML. `gen-fabric` writes the default catalog beside its output and selects
`basic_alu` by default. It accepts
`--compute-catalog` and `--compute-profile` to select another catalog/profile.

The platform declares named operation classes. A class lists instruction names
with a shared input arity and FLOP-equivalent weight.
Instruction names are lowercase in both the class and program calls.
Each PE then declares its engines and the rate at which each engine executes a
class. No compute capability is assumed when it is absent from the platform.

```yaml
operation_classes:
  arithmetic: { operations: [add, mul], arity: 2 }
  matrix: { operations: [matrix_mac], arity: 2, flops_per_op: 2 }
  trig: { operations: [sin, cos, tan], arity: 1 }
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      compute:
        engines:
          vector:
            capabilities:
              - { class: arithmetic, input_dtypes: [[fp16, fp16], [bf16, bf16]], ops_per_tick: 32 }
              - { class: trig, ops_per_tick: 32 }
          matrix:
            capabilities:
              - { class: matrix, ops_per_tick: 128 }
```

`input_dtypes` contains exact, ordered combinations of length `arity`.
Omitting it matches combinations that have no explicit entry for that
operation. An explicit entry wins over a fallback even when it is on another
engine. If several engines match the selected declaration, a batch takes the
first available engine; independent batches can use those engines concurrently.
Output datatypes come from the timetable's tensor and temporary
declarations; they do not affect engine selection. The simulator currently
does not validate whether an operation's output datatype is appropriate for
its input datatypes. `flops_per_op` defaults to one; matrix MACs commonly use two.

A program can call any declared instruction using
`{ sin: [{ read: { tensor: A, indices: [i] } }] }`.
The program derives input datatypes from its tensors. The simulator tracks
typed work and movement; it does not calculate output values. Named operation
counts and weighted FLOP equivalents appear in statistics.

An engine runs one batch at a time. Operations assigned to one engine share
its throughput, while independent engines overlap. For an engine batch, the
service time is `ceil(sum(count / ops_per_tick))`. A `matrix_mac` expression
requires a matching capability, including its explicit execution mode when
supplied. Fused temporaries stay in SRAM. A tensor edge between compute nodes
requires the full producer node to complete before consumers proceed.

### Execution modes and quantized storage

Typed capabilities may additionally select an explicit execution mode. The
following fragment describes BF16 matrix operands with a tensor instruction
and an FP32 accumulator:

```yaml
- class: matrix
  input_dtypes: [[bf16, bf16]]
  ops_per_tick: 128
  execution: { instruction: tensor, accumulator: fp32 }
```

Match that mode in the program's `matrix_mac` expression with
`execution: { instruction: tensor, accumulator: fp32 }`. Conversion to an
output's narrower datatype is accounted for separately and needs the
corresponding `convert` capability. Storage dtype alone does not establish
an executable quantized matrix mode: a workload must declare scale metadata
and unpack/dequantize work where required by its chosen lowering. Program
`metadata_inputs` are full-view boundary inputs loaded for each output tile
even when arithmetic does not index them.

### Block execution resources

A PE that accepts timetable `pipeline` requests explicitly declares:

```yaml
block_execution:
  max_compute_tiles: 1
  max_load_chunks: 1
  max_store_tiles: 1
  transfer_port: lsu
  accumulator_storage: shared_sram
  # Optional shared combined-direction local staging bandwidth:
  # local_memory_service: {bytes_per_tick: 256}
```

These are PE-wide admission limits shared by all pipeline nodes. The initial
implementation requires all three limits to equal one and supports the LSU
route and shared SRAM backing shown above. Timetable buffer counts are
independent of these limits. Generic programs use a separate working arena
for operand copies, temporaries, reduction state and current results, alongside
all requested input/output slots. The native matrix accumulator occupies the SRAM window once
and also requires retained capacity on its selected matrix engine; retaining
capacity does not keep that engine busy between compute steps.

Without `local_memory_service`, local storage capacity and lifetimes are
modelled but local staging bandwidth is unmodelled. When selected, one byte
service is shared across input staging writes, operand reads, accumulator
readout, final output writes and output drains. Generic paths also charge the
slot-to-working-arena and working-arena-to-output copies they actually perform. This does not add external
traffic or charge per-MAC accumulator accesses. Its rate must be positive
and qualified by the scenario. It cannot coexist with source-specific
`local_payload_service` until their resource sharing is defined.

## Modelling a machine

Begin with physical ownership and address ranges: assign memories to PEs,
declare the addressable devices in each memory map, and connect local and
remote paths explicitly. A memory map makes storage addressable; the
connections determine which links and service resources requests traverse.
Then declare compute classes, operand types, execution modes, engine rates,
LSU request sizes, SRAM admission capacity, and memory/link rates and delays.
State the clock period used to convert rates per tick to rates per second.

For specific topologies, workload experiments, provenance, and calibration
workflows, see the [`gwr-use-cases` package][gwr-use-cases]. It owns
the analytic GB200 references and model generators built on this library.

<!-- ANCHOR_END: compute_engines -->

See [the matrix/vector example](examples/compute_engines.yaml).

[gwr-use-cases]: ../gwr-use-cases/README.md

### Address-to-memory striping

Memory maps default to contiguous physical placement. An optional policy spreads
successive stripes across an explicitly ordered group of physical memories:

```yaml
memory_maps:
  - name: local
    devices: [hbm0, hbm1, ddr0]
    address_mapping:
      mode: striped # change only this to contiguous for a matched baseline
      stripe_bytes: 64
      groups:
        - name: hbm
          devices: [hbm0, hbm1]
```

Groups must contain at least two distinct equal-capacity memories whose physical
windows form one adjacent span. Bases and capacities must align to the power-of-two
stripe size, which must also be a multiple of every applicable LSU fetch and cache
line size. Group settings remain present and validated in `contiguous` mode.
Ungrouped memories keep contiguous placement. HBM and DDR can therefore use
different policies without changing tensor addresses or capacity.

For a logical offset `a` within a group of `N` memories and stripe size `S`, the
destination is `(a / S) % N` and its local offset is `(a / (S*N))*S + a%S`.
Requests carry the translated absolute physical address through the existing
cache and fabric models. Cache indexing consequently uses physical addresses;
changing placement may also change cache set conflicts. Clients sharing data
must select equivalent mappings. Direct atomics in actively striped windows are
currently rejected during preflight; contiguous and ungrouped atomics retain
their existing behavior. Striping changes placement, not configured bandwidth.
