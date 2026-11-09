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

processing_elements:
  - name: pe0
    memory_map: pe_memory_map
    config:
      lsu_access_bytes: 32
      sram_bytes: 64KiB

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
