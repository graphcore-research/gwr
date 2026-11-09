<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# gwr-models

<!-- ANCHOR: overview -->

Models are the constructed from [components] and [resources] and run by the
[`gwr_engine`].

The `gwr_models` library provides a collection of connectable models to be used
when building larger models and simulations. The following is a brief and not
exhaustive list of the models provided.

## Frames

Many systems are based on frames (packets) of different forms.

### Memory Access

Memory traffic is represented with `MemoryAccess`, which carries routing, access
type, payload size, and protocol overhead.

### Ethernet Frame

The `EthernetFrame` represents a frame that looks like the one defined in the
standards.

## Flow Controlled Pipeline

A flow controlled pipeline represents a low-level hardware component which can
be used to moved data in a system. It comprises a buffer that will hold data
received from the sender and a credit-based mechanism for ensuring the buffer
doesn't overflow.

**Interfaces:** `rx`: [input port], `tx`: [output port]

## Ethernet Link

The `EthernetLink` is effectively a bi-directional set of connections that have
similar properties to a connection over an Ethernet link.

**Interfaces:** `rx_a`, `rx_b` : [input port]s, `tx_a`, `tx_b`: [output port]s

## Memory

A model of a memory to handle read/write accesses.

**Interfaces:** `rx`: [input port], `tx`: [output port]

## Cache

A whole-line n-way set associative cache with bounded parent requests and line
fills. The implementation and constructors live in `cache`; ordinary packets
need only implement `CacheAccess`.

In coherent and incoherent caches, parents completed by memory responses use a
separate return worker and retain admission through response service.
Outstanding fills can finish ingress while the requesting device is blocked,
with completed parents bounded by the admission limit. Forwarded nonposted-write
acknowledgements also use the return worker; their ingress waits for completion,
allowing at most one extra queued response.

`Cache::new_coherent_and_register` additionally takes a `CoherenceConfig` and
requires `CoherentAccess`. The same cache core then models directory-managed
shared/exclusive permissions, dirty writeback, invalidations, and scoped
barriers. `cache::coherency_manager` provides the directory endpoint. Cache and
manager line sizes must match; platform construction validates this
relationship.

Barriers carry a destination address. When coherence is enabled, the cache
drains its prior requests, then forwards the barrier to the coherence manager
responsible for that address, as selected by the manager map in
`CoherenceConfig`. The cache waits for that manager's acknowledgement before
completing the barrier and processing later device requests. The selected
manager drains its prior operations across all address ranges managed by it. A
barrier is scoped to the receiving cache and the selected manager; it is not
broadcast to other managers and does not flush resident dirty cache lines.

Coherence managers process work without a clock-rate limit by default. Use
`CoherencyManagerConfig::with_operations_per_tick(n)` to set a positive shared
processing budget. Handling an incoming packet, starting a queued request, or
completing a barrier each consumes one operation. Requests that initially wait
for a line or barrier consume another operation when they are replayed. The
budget resets each clock tick; unused operations do not accumulate.

Responses, writebacks, and invalidation acknowledgements have priority over
ordinary requests and queued replay work, but consume the same budget. Ingress
and output run independently of processing, so output backpressure does not
prevent the manager from handling protocol traffic. The limit controls directory
processing, not queue capacity or packet bandwidth. Generating several
invalidations is one processing operation; fabric bandwidth limits their
transmission.

**Interfaces:**

- `dev_rx`: device-side [input port]
- `dev_tx`: device-side [output port]
- `mem_rx`: memory-side [input port]
- `mem_tx`: memory-side [output port]

## Ring Node

A model of a node that can sit in a ring communication topology.

**Interfaces:**

- `ring_rx`: [input port] for data travelling in ring
- `ring_tx`: [output port] for data travelling in ring
- `io_rx`: [input port] for data entering the ring
- `io_tx`: [output port] for data leaving the ring

## Fabric

A model of a two-dimensional interconnect fabric. It is provided in both
functional and routed implementations that provide the same interfaces but trade
off model accuracy vs run-time performance.

**Interfaces:**gwr-

- `rx(i)`: [input port] for data ingress into the fabric
- `tx(i)`: [output port] for data egress from the fabric

<!-- ANCHOR_END: overview -->

[components]: ../gwr-components/README.md
[resources]: ../gwr-resources/README.md
[`gwr_engine`]: ../gwr-engine/README.md
[input port]: ../gwr-engine/src/port/mod.rs
[output port]: ../gwr-engine/src/port/mod.rs

<!-- ANCHOR: testing -->

## Testing

Models can be tested using the `build_model_harness!` macro. This wraps a model
with a simple test harness that drives and expects objects that implement the
`AccessMemory` trait. The model harness uses the same harness DSL and generated
API as `build_component_harness!`, but uses `MemoryTxn` to check objects coming
out of the model.

Cache models require `CacheAccess`, which extends `AccessMemory` with allocation
policy and fill ownership. Declare a cache harness as
`harness CacheHarness<T: gwr_models::memory::traits::CacheAccess>`; ordinary
memory harnesses can continue to use `harness MemoryHarness<T>`.

`MemoryTxn` lets tests match only the memory access fields that matter for a
scenario. For example, this test harness checks that a `Memory` model produces a
read response for the expected destination address, while ignoring fields that
are not specified:

```rust,no_run
mod memory_harness {
    use std::rc::Rc;

    use gwr_engine::test_helpers::start_test;
    use gwr_models::build_model_harness;
    use gwr_models::memory::memory_access::MemoryAccess;
    use gwr_models::memory::{Memory, MemoryConfig};
    use gwr_models::test_helpers::{MemoryTxn, create_default_memory_map, create_read};

    const DST_ADDR: u64 = 0x80000;
    const SRC_ADDR: u64 = 0x90000;
    const ACCESS_SIZE_BYTES: usize = 64;
    const OVERHEAD_SIZE_BYTES: usize = 8;

    build_model_harness! {
        harness MemoryHarness<T> {
            component: memory: Rc<Memory<T>>,
            rx ports: {
                Rx<T> => rx,
            },
            tx ports: {
                Tx<T> => tx,
            },
        }
    }

    #[test]
    fn model_harness_matches_selected_memory_fields() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let config = MemoryConfig::new(DST_ADDR, 0x40000, 32, 1);
        let memory = Memory::<MemoryAccess>::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "memory",
            config,
        )
        .unwrap();
        let memory_map = Rc::new(create_default_memory_map());
        let request = create_read(
            engine.top(),
            &memory_map,
            ACCESS_SIZE_BYTES,
            DST_ADDR,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );
        let mut harness = MemoryHarness::new(engine, memory);

        harness.run_steps([
            send_rx!(request),
            expect_tx!(MemoryTxn::read_rsp(DST_ADDR)),
        ]);
    }
}
```

Use the component harness documentation for the shared syntax and execution
model.

<!-- ANCHOR_END: testing -->
