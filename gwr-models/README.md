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

A basic model of a n-way set associative cache.

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
[input port]: ../gwr-developer-guide/md_src/components/ports.md#input-ports
[output port]: ../gwr-developer-guide/md_src/components/ports.md#output-ports

<!-- ANCHOR: testing -->

## Testing

Models can be tested using the `build_model_harness!` macro. This wraps a model
with a simple test harness that drives and expects objects that implement the
`AccessMemory` trait. The model harness uses the same harness DSL and generated
API as `build_component_harness!`, but uses `MemoryTxn` to check objects coming
out of the model.

`MemoryTxn` lets tests match only the memory access fields that matter for a
scenario. For example, this test harness only checks the destination address
when the `step_expect_tx()` is called:

```rust,no_run
mod delay_harness {
    use std::rc::Rc;

    use gwr_components::delay::Delay;
    use gwr_engine::test_helpers::start_test;
    use gwr_models::build_model_harness;
    use gwr_models::memory::memory_access::MemoryAccess;
    use gwr_models::test_helpers::{MemoryTxn, create_default_memory_map, create_read};

    const DST_ADDR: u64 = 0x80000;
    const SRC_ADDR: u64 = 0x90000;

    build_model_harness! {
        harness DelayHarness<T> {
            component: delay: Rc<Delay<T>>,
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
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1).unwrap();
        let memory_map = Rc::new(create_default_memory_map());
        let access = create_read(engine.top(), &memory_map, 64, DST_ADDR, SRC_ADDR, 8);
        let mut harness = DelayHarness::new(engine, delay);

        harness.run_steps([
            step_send_rx(access),
            step_expect_tx(MemoryTxn::read_req(DST_ADDR)),
        ]);
    }
}
```

Use the component harness documentation for the shared syntax and execution
model.

<!-- ANCHOR_END: testing -->
