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
configuration. Coherent backing maps must use contiguous addresses. Aggregate roofline
analysis does not model cache coherence.

The coherent examples demonstrate one and two managers. `gen-fabric` accepts
`--use-coherency-managers` with L1 caches and no L2 cache.

## Preflight and Atomic Construction

`PreparedPlatform::new(&config)` validates a platform and resolves device IDs,
address maps, and fabric destination routes without creating an engine or any
models. Address maps come from validated memory configuration, not from detailed
memory instances. The prepared object borrows the immutable configuration until
it is consumed by `build`:

```rust
use gwr_engine::engine::Engine;
use gwr_engine::types::DeviceId;
use gwr_platform::PreparedPlatform;
use gwr_platform::types::PlatformConfig;

fn main() -> Result<(), Box<dyn std::error::Error>> {
let config: PlatformConfig = yaml_serde::from_str(
    "memory_maps:
  - name: global
    devices: [ram]
memories:
  - name: ram
    kind: hbm
    base_address: 4096
    config: {capacity_bytes: 4096}
",
)?;
let prepared = PreparedPlatform::new(&config)?;
assert_eq!(prepared.device_id("ram")?, DeviceId(0));
assert_eq!(prepared.memory_map("global")?.lookup(4100), Some((DeviceId(0), 4)));
let mut engine = Engine::default();
let clock = engine.default_clock();
let platform = prepared.build(&engine, &clock)?;
assert_eq!(platform.num_memories(), 1);
// This example inspects construction only: the memory has no connected client.
Ok(())
}
```

`Platform::from_file` and `Platform::from_string` use the same path. Building
stages all model registrations in an engine registration transaction, including
nested constructor registrations. It commits only after all ports are connected;
an error discards the staged runnables and callbacks without disturbing existing
engine work. Entity IDs and trace construction records are not rolled back. Do
not wrap these APIs in another registration transaction: nesting is rejected.
Low-level `builder` functions retain their existing independent registration
behavior.

Construction supports detailed platforms, including cache chains and
deterministic routes to multiple memories. Timetable files and the dispatch
interface are unchanged.

## Roofline analysis

Use `gwr-timetable --roofline-analyze` to estimate a timetable directly from a
normal platform file. The analyzer applies roofline fidelity to an in-memory
copy of the platform configuration, so the source YAML stays unchanged.

```bash
cargo run --release -p gwr-timetable --bin gwr-timetable -- \
  --roofline-analyze \
  --platform examples/platform_4x4.yaml \
  --timetable ../gwr-timetable/examples/small.yaml
```

The command always prints a completion line with estimated simulated time. Pass
`--stdout` or `--dump-stats` to enable the Info-level summary with system totals
and per-PE, per-memory, and per-component details. It is an aggregate
approximation for performance trends; it does not run the event simulator or
emit a Perfetto trace. Calibration should compare its estimates against detailed
runs of the same workloads and platform.

The analyzer uses the platform's resource and route configuration in memory; the
platform file remains the ordinary detailed-model configuration.

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

## Independent routed paths

Routed fabrics configure one bandwidth per directed path. External ingress and
egress ports buffer and apply backpressure without separate rate limiters. Each
plane has independent link queues and arbitration; opposite directions also have
independent service.

```yaml
config:
  paths:
    count: 4
    bw_bits_per_tick: 435
    selection: { policy: address-xor-fold-v1, address_shift: 6 }
    queue_bytes: 8192
    queue_packets: 256
```

Rates use the existing simulator clock; this does not add frequency settings. At
1 ns per tick, this example supplies 54.375 GB/s per path before contention.
Service retains fractional packet timing debt, so a 64-byte packet consumes
512/435 ticks rather than two separately rounded ticks. Hop delays keep their
existing units, and queue capacity is independent of hop latency. Each hop
forwards the packet head after that hop's configured propagation latency while
reserving bandwidth for the entire packet. Exact head/tail availability follows
the packet, so equal-rate uncongested hops overlap serialization rather than
repeating it. Protocol clients receive the packet only after its tail arrives.
Byte and packet credits remain owned through both tail completion and downstream
ACK, including when a completed packet is buffered under backpressure. Local
delivery uses the same route bandwidth; there is no additional external-port
rate cap.

The XOR-fold policy supports power-of-two path counts and uses the final
memory's stable configured ordinal and local physical address after memory
striping. Ignoring six address bits keeps an aligned 64-byte region on the same
plane for all PEs and request/reply directions. Memory packets on multiple
planes must fit within that region. Same-stream order is preserved; independent
PE streams acquire an order where they converge. `address-mix-modulo-v1`
supports arbitrary positive path counts without promising exact balance for
small address blocks. Gateway attachments use the same canonical region, so
overlapping packets do not split across attachments. Routed fabrics reject
source-dependent attachment selection. Rust callers can inject a pure selector
over the same canonical address key; multi-plane construction requires an
explicit protocol adapter.

Both byte and packet queue bounds must be positive, including for zero-byte
modeled request headers. Queue credit is retained through actual downstream
acknowledgement. `route_snapshots()` reports transport bytes, packets, retained
ownership, service time and downstream blocked time per named directed route.
Memory packets also report separate payload and header bytes; these counters are
unknown for generic packet types that do not expose that distinction. Transport
counts include protocol headers and each hop; memory payload counters remain
separate. Snapshots do not equate transport byte-hops with HBM bandwidth.

Routed configurations reject `port_bits_per_tick` and
`internal_link_bits_per_tick`; migrate to `paths.bw_bits_per_tick`. Omitting
`paths` selects the default single path at 256 bits/tick. Functional fabrics
keep their existing port-rate configuration and reject `paths`.

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

Capabilities can live in a catalog shared by multiple platform files. A catalog
contains `operation_classes` and `compute_profiles`; each profile is the
complete engine configuration for one PE. For example,
`examples/default_compute.yaml` defines `basic_alu` with 16 adds, 4 multiplies,
and 16 compares per tick on its ALU, plus a separate engine for 32 matrix MACs
per tick. Its `wide_alu` profile has a separate matrix engine for 256 MACs per
tick:

```yaml
includes: [default_compute.yaml]
processing_elements:
  - name: pe0
    memory_map: mm
    config: { compute: basic_alu }
```

This is a catalog/PE configuration fragment. A runnable platform also needs the
`mm` memory map, memories, and connections, as in the example above.

`includes` paths are relative to the file that declares them. Catalogs can
include other catalogs. Re-importing the same file is harmless; duplicate
definition names, include cycles, missing files, and unknown profile names are
errors. A platform may also define its own classes and profiles alongside its
topology, or put an inline `engines` map in a PE's `compute` field. Use
`Platform::from_file` for includes; `from_string` accepts only self-contained
YAML. `gen-fabric` writes the default catalog beside its output and selects
`basic_alu` by default. It accepts `--compute-catalog` and `--compute-profile`
to select another catalog/profile.

The platform declares named operation classes. A class lists instruction names
with a shared input arity and FLOP-equivalent weight. Instruction names are
lowercase in both the class and program calls. Each PE then declares its engines
and the rate at which each engine executes a class. No compute capability is
assumed when it is absent from the platform.

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
              - {
                  class: arithmetic,
                  input_dtypes: [[fp16, fp16], [bf16, bf16]],
                  ops_per_tick: 32,
                }
              - { class: trig, ops_per_tick: 32 }
          matrix:
            capabilities:
              - { class: matrix, ops_per_tick: 128 }
```

`input_dtypes` contains exact, ordered combinations of length `arity`. Omitting
it matches combinations that have no explicit entry for that operation. An
explicit entry wins over a fallback even when it is on another engine. If
several engines match the selected declaration, a batch takes the first
available engine; independent batches can use those engines concurrently. Output
datatypes come from the timetable's tensor and temporary declarations; they do
not affect engine selection. The simulator currently does not validate whether
an operation's output datatype is appropriate for its input datatypes.
`flops_per_op` defaults to one; matrix MACs commonly use two.

A program can call any declared instruction using
`{ sin: [{ read: { tensor: A, indices: [i] } }] }`. The program derives input
datatypes from its tensors. The simulator tracks typed work and movement; it
does not calculate output values. Named operation counts and weighted FLOP
equivalents appear in statistics.

An engine runs one batch at a time. Operations assigned to one engine share its
throughput, while independent engines overlap. For an engine batch, the service
time is `ceil(sum(count / ops_per_tick))`. A `matrix_mac` expression requires a
matching capability, including its explicit execution mode when supplied. Fused
temporaries stay in SRAM. A tensor edge between compute nodes requires the full
producer node to complete before consumers proceed.

### Execution modes and quantized storage

Typed capabilities may additionally select an explicit execution mode. The
following fragment describes BF16 matrix operands with a tensor instruction and
an FP32 accumulator:

```yaml
- class: matrix
  input_dtypes: [[bf16, bf16]]
  ops_per_tick: 128
  execution: { instruction: tensor, accumulator: fp32 }
```

Match that mode in the program's `matrix_mac` expression with
`execution: { instruction: tensor, accumulator: fp32 }`. Conversion to an
output's narrower datatype is accounted for separately and needs the
corresponding `convert` capability. Storage dtype alone does not establish an
executable quantized matrix mode: a workload must declare scale metadata and
unpack/dequantize work where required by its chosen lowering. Program
`metadata_inputs` are full-view boundary inputs loaded for each output tile even
when arithmetic does not index them.

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
implementation requires all three limits to equal one and supports the LSU route
and shared SRAM backing shown above. Timetable buffer counts are independent of
these limits. Generic programs use a separate working arena for operand copies,
temporaries, reduction state and current results, alongside all requested
input/output slots. The native matrix accumulator occupies the SRAM window once
and also requires retained capacity on its selected matrix engine; retaining
capacity does not keep that engine busy between compute steps.

Without `local_memory_service`, local storage capacity and lifetimes are
modelled but local staging bandwidth is unmodelled. When selected, one byte
service is shared across input staging writes, operand reads, accumulator
readout, final output writes and output drains. Generic paths also charge the
slot-to-working-arena and working-arena-to-output copies they actually perform.
This does not add external traffic or charge per-MAC accumulator accesses. Its
rate must be positive and qualified by the scenario. It cannot coexist with
source-specific `local_payload_service` until their resource sharing is defined.

## Modelling a machine

Begin with physical ownership and address ranges: assign memories to PEs,
declare the addressable devices in each memory map, and connect local and remote
paths explicitly. A memory map makes storage addressable; the connections
determine which links and service resources requests traverse. Then declare
compute classes, operand types, execution modes, engine rates, LSU request
sizes, SRAM admission capacity, and memory/link rates and delays. State the
clock period used to convert rates per tick to rates per second.

Keep application-specific topologies, workload generators, provenance and
calibration evidence in the application workspace using this library.

<!-- ANCHOR_END: compute_engines -->

See [the matrix/vector example](examples/compute_engines.yaml).


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
windows form one adjacent span. Bases and capacities must align to the
power-of-two stripe size, which must also be a multiple of every applicable LSU
fetch and cache line size. Group settings remain present and validated in
`contiguous` mode. Ungrouped memories keep contiguous placement. HBM and DDR can
therefore use different policies without changing tensor addresses or capacity.

For a logical offset `a` within a group of `N` memories and stripe size `S`, the
destination is `(a / S) % N` and its local offset is `(a / (S*N))*S + a%S`.
Requests carry the translated absolute physical address through the existing
cache and fabric models. Cache indexing consequently uses physical addresses;
changing placement may also change cache set conflicts. Clients sharing data
must select equivalent mappings. Direct atomics in actively striped windows are
currently rejected during preflight; contiguous and ungrouped atomics retain
their existing behavior. Striping changes placement, not configured bandwidth.

## Named platform composition

A platform file can instantiate other platform files. Use `includes` for shared
operation classes/compute profiles, and `instances` for independent hardware.
The example in `examples/composition/system.yaml` builds two racks, four boards
and eight accelerator packages from four reusable definitions.

```yaml
name: accelerator_board
level: board
instances:
  - name: chip
    file: package.yaml
    count: 8
    address_base: 0
    address_stride: 16GiB
    memory_map_bindings: { remote_dma: board_memory }
memory_maps:
  - name: board_memory
    devices: ["chip[0]/hbm", "chip[1]/hbm"] # list the desired memories
connections:
  - connect: ["chip[0]/uplink", "chip[1]/fabric.noc@(0,0).3"]
```

Paths resolve relative to the file declaring them. Every instance has a stable
name and zero-based index, including singletons: `chip[0]/cpu`. Descendant paths
compose, for example `rack[1]/board[0]/chip[3]/cpu`. Local identifiers use
letters, digits and underscores. Files are loaded once as definitions, but every
instance creates independent hardware. Catalog includes retain their existing
deduplication.

An explicit hierarchy obeys these ownership rules:

| Parent    | Direct children                                             |
| --------- | ----------------------------------------------------------- |
| system    | rack                                                        |
| rack      | board                                                       |
| board     | package or daughterboard (`board`)                          |
| package   | component definitions, or local PEs/caches/memories/fabrics |
| component | local PEs/caches/memories/fabrics                           |

A package, board or rack may be the root of a reusable fragment. Its parent
constraints apply when imported. Hardware at system/rack/board level is
rejected. A network switch is a package containing fabric hardware; connections
can cross any ownership boundary. HBM belongs to a compute or memory package;
DDR is a separate memory package on a board (a DIMM can be a daughterboard).
Silicon mapping retains the separate DDR-versus-compute-package check. Legacy
flat files remain valid logical-only platforms; they do not acquire invented
package owners.

Logical hierarchy expresses ownership. Applications can attach physical
dimensions and placements to the resolved logical names, for example
`pe.rack[0]/board[0]/chip[0]/cpu.engine.alu`.

### Free ports and aliases

A child can optionally expose convenient names:

```yaml
ports:
  uplink: fabric.noc@(0,0).3
```

An ancestor can connect `chip[0]/uplink`, or directly name any free descendant
port such as `chip[0]/fabric.noc@(0,0).3`. Aliases resolve recursively.
Child-internal connections are retained; a concrete port can be used only once,
even through different aliases or `.compute`/default-port spellings. An endpoint
connected to `'null'` is deliberately disabled and cannot be reused. Unlisted
free ports can be wired by an ancestor. Prefer explicit `.dev`/`.mem` for cache
ports.

A fragment may leave ports and imported memory-map references unresolved. The
final materialized hierarchy validates native devices, maps, connection pairs,
port ownership, address ranges and request paths. Building a simulator still
requires all runnable ports to be connected or explicitly disabled.

### Address relocation and access scope

There is one global **64-bit byte address space** (16 EiB). Definitions use
local memory addresses. Each parent relocates a child using checked arithmetic:

```text
child_origin = parent_origin + address_base + index * address_stride
global_memory_base = child_origin + local_memory_base
```

The root origin is zero. `address_base`/`address_stride` accept integers, hex
and byte-unit strings. Repeated memory-bearing instances require a positive
stride; there is no automatic packing. Overflow, overlap, incompatible stripe
alignment and fetch alignment are rejected. Windows retain local and global
addresses in `MaterializedPlatform.memory_windows`.

Every local component, memory map, operation class and compute profile is scoped
to its instance. References follow that scope. Local PE/cache defaults are
applied before fragments are combined; parent defaults do not modify imported
children.

A child PE can name a map supplied by its parent. The instance explicitly binds
that unresolved name using `memory_map_bindings`. Binding an existing concrete
child map is an error; there is no implicit ancestor-map lookup. Parent maps can
list descendant memories. A narrow `memory_map` restricts compute LSU accesses;
an optional wider `dma_memory_map` allows explicit staging through the PE's
connected `.dma` interface. Both require real paths through the topology. Memory
sharing grants access, not coherence or an implicit copy. Timetable tensor
addresses are resolved global addresses; `kind: transfer` expresses data
movement. The example `transfer.yaml` stages 64 bytes from a neighbouring
package into local HBM through a board-scoped DMA map.

### Compact connection loops

`count` repeats instances. `repeat_connections` repeats endpoint pairs without
general YAML templating:

```yaml
repeat_connections:
  - for: { i: [0, 7] }
    connect: ["chip[${i}]/right", "chip[${i + 1}]/left"]
```

Ranges are half-open integer intervals (`[0,7]` emits indices 0 through 6).
Multiple variables produce deterministic nested loops in variable-name order.
Index substitutions support integer literals, loop variables, parentheses and
checked `+`, `-`, `*`. No arbitrary code, conditionals, division, or
substitutions in configurations, capacities or names are evaluated. Invalid
expressions, negative indices and oversized expansions fail. Each rule is
bounded to 100,000 connections.

### Resolution and scale

`gwr_platform::composition::load_source(path)` returns a held `SourceGraph` with
compressed counted instances and SHA-256 hashes of all transitive platform and
catalog inputs. It does not create one object per repeated leaf.
`materialize(limit)` produces a native `PlatformConfig` YAML snapshot and memory
windows. The default file loader allows 100,000 expanded objects, including
fabric tile ports. `Platform::from_file`, silicon inventory and the platform
viewer use this resolver. File-dependent imports require `from_file`;
`from_string` accepts native snapshots.

The viewer uses the compressed graph, pages instance arrays, and drills from the
highest declared scope down to individual components. It preserves u64 addresses
as strings in browser JSON. Native simulation and silicon costing intentionally
materialize bounded topologies. Architecture-exploration candidates archive the
original source hierarchy and its transitive hashes in authenticated candidate
metadata, alongside their independent flat native replay snapshot.
