<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Examples

GWR includes a number of example applications that demonstrate how components
can be written and used for architectural exploration.

- The [Flaky Component](#flaky-component) shows how to write a component with
  custom functionality.
- The [Flaky with Delay](#flaky-with-delay) extends this to show how to mix
  library components and custom functionality.
- The [Scrambler](#scrambler) shows how to remap component ports.
- The [Pipe simulation](#sim-pipe) provides a command-line application to
  explore the flow-controlled pipeline.
- The [Ring simulation](#sim-pipe) provides a command-line application to
  explore a ring-based interconnect.
- The [Fabric simulation](#sim-pipe) provides a command-line application to
  explore a rectangular fabric interconnect.

## Flaky Component

Here is an example of a full worked main with command-line argument parsing that
uses the [component] created in the [writing a component section].

```rust,ignore
{{#rustdoc_include ../../../examples/flaky-component/src/main.rs}}
```

## Flaky with Delay

Building on the [Flaky Component](#flaky-component), this example adds an
internal delay and a buffer for any packets that aren't dropped.

## Scrambler

The Scrambler is a very simple component that shows how it is possible to re-map
the port connections in a component as the model is being built.

## Sim Pipe

A basic application to show how to explore the performance trade offs of a
flow-controlled pipeline. It provides all the command-line options necessary to
configure all aspects of the flow-controlled pipeline and see the impact on
performance.

## Sim Ring

A slightly more complex application that will build a ring-based interconnect.
It allows the user to explore the performance of the ring and the impact that
the priority in the arbiter makes.

For more details, see the documentation in the source code.

## Sim Fabric

The fabric simulation allows the user to explore the performance of different
sized rectangular fabrics with many different properties that can be configured.
It is also a demonstration of how it is possible to have models at different
levels of abstraction. The default is to use a functional model that doesn't
bother with all the internal details of the fabric. However, using `--routed`
changes to the model that implements all the internal routing and arbitration
stages required to build a fabric.

For more details, see the documentation in the source code.

[component]: ../components/chapter.md
[writing a component section]: ../components/writing_a_component.md
