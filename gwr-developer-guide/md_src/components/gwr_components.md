<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# The gwr_components Library

This page details the basic components that the `gwr_components` library
provides.

## Data Sources and Sinks

### Source

The `Source` component will drive objects into any other component. It is
configured with a generator to produce the objects.

**Interfaces:** `tx`: [output port].

### Sink

The `Sink` component will pull objects from another component. It keeps track of
the number of sunk objects for basic checking.

**Interfaces:** `rx`: [input port].

## Data Store

The `Store` is a basic component that can store a specified number of objects.

**Interfaces:** `rx`: [input port], `tx`: [output port].

## Delay

The `Delay` component adds a defined delay (in [`Clock`] ticks) from the time an
object enters it until it is then sent on.

**Interfaces:** `rx`: [input port], `tx`: [output port].

## Rate Limiters

The `Limiter` component models how long it takes for objects to travel through
them and ensure their bandwidth limits are respected.

**Interfaces:** `rx`: [input port], `tx`: [output port].

## Routers

A `Router` takes objects from its input and sends them to one of its `i` outputs
depending on the `dest()` it provides.

**Interfaces:** `rx`: [input port], `tx(i)`: [output ports].

## Arbiters

An `Arbiter` takes objects from one of its `i` inputs and sends them to its
output. The `Arbiter` is created with a defined policy as to how to choose
between its inputs if more than one of them is ready.

**Interfaces:** `rx(i)`: [input ports], `tx`: [output port].

## Arbiteration Policies

An `Arbiter` needs to be created with an arbitration policy in order to make the
arbitration decisions. A number of arbitration policies are provided in the
library or the user can write their own custom policy. The existing policies
are:

- `Round Robin`
- `Weighted Round Robin`
- `Priority Round Robin`

[`Clock`]: ../gwr_engine/time/clocks.md
[input port]: ../components/ports.md#input-ports
[input ports]: ../components/ports.md#input-ports
[output port]: ../components/ports.md#output-ports
[output ports]: ../components/ports.md#output-ports
