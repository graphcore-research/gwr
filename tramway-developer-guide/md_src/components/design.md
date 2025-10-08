<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Design the Component

There are a number of things to consider when designing a new simulation
component. The two main aspects are

1. [What are the interfaces to other components](#component-interfaces)
1. [What functionality will it have internally](#component-functionality)

## Component Interfaces

An interface will comprise one or more [ports] and define how a component
connects to and interacts with other components.

So it is first essential to define the types of interfaces a component will have
and how many of each there will be. Then, the required ports can be created.

A port has a flow of data. The general naming convention is:

- Where data flows _in_ to a component it is a receive port (`rx`).
- Where data flows _out_ of a component it is a transmit port (`tx`).

## Component Functionality

Some components are simply collections of other components plugged together. In
most cases, however, it will be necessary to define custom functionality for the
port. This includes how the ports handle data they send/receive as well as
general activity that can be [spawned] in the [`run()`] function.

[ports]: ../components/ports.md
[`run()`]: ../components/custom_functionality.md
[spawned]: ../tramway_engine/chapter.md#spawner
