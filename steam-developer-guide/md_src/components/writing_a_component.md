<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Examples

This example shows the source required to implement a custom [component]. This
example talks through create a component that will drop a specified rate of
packets.

## Use Statements

The first thing to do is `use` all the libraries that are required.

```rust,ignore
{{#rustdoc_include ../../../examples/flaky-component/src/lib.rs:use}}
```

## Struct

Next, a `struct` representing the state of the component needs to be defined.

```rust,ignore
{{#rustdoc_include ../../../examples/flaky-component/src/lib.rs:struct}}
```

## State

A component usually requires a `State` in order to support any mutable state.
That is therefore declared next:

```rust,ignore
{{#rustdoc_include ../../../examples/flaky-component/src/lib.rs:state}}
```

## Component Implementation

The component itself needs to implement a number of functions, including the
constructor (`new()`) and functions that allow it to be connected
(`connect_port_tx()` / `port_rx()`):

```rust,ignore
{{#rustdoc_include ../../../examples/flaky-component/src/lib.rs:implFlaky}}
```

{{#include ../links_depth1.md}}
