<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Models

Models are the constructed from [components] and [resources] and run by the
[`gwr_engine`].

The `gwr_models` library provides a collection of connectable models to be used
when building larger models and simulations. The following is a brief and not
exhaustive list of the models provided.

## Frames

Many systems are based on frames (packets) of different forms.

### Data Frame

In order to model different protocols an abstract `DataFrame` is provided. It
has a configurable payload and overhead sizes.

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

**Interfaces:**

- `rx(i)`: [input port] for data ingress into the fabric
- `tx(i)`: [output port] for data egress from the fabric

[components]: ../components/chapter.md
[resources]: ../resources/chapter.md
[`gwr_engine`]: ../gwr_engine/chapter.md
[input port]: ../components/ports.md#input-ports
[input ports]: ../components/ports.md#input-ports
[output port]: ../components/ports.md#output-ports
[output ports]: ../components/ports.md#output-ports
