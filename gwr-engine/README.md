<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# gwr-engine

<!-- ANCHOR: overview -->

`gwr_engine` is a single-threaded asynchronous simulation engine designed to run
models of asynchronous simulation [components].

The engine can run purely event-driven models, clocked models, or simulations
that combine both styles. Components are registered with the engine before
execution, ports are connected before a run starts, and clocks provide modeled
time for tasks that need deterministic delays.

## Features

- `global_allocator`: When enabled, applications that depend on `gwr-engine` use
  a global allocator selected to deliver strong runtime performance for the GWR
  engine. This is currently [mimalloc](https://github.com/microsoft/mimalloc).

  This feature is enabled by default. Applications that need a different global
  allocator must disable it explicitly.

## Developer Guide

The Developer Guide provides a directed explanation of the GWR engine and
related libraries. See the `gwr-developer-guide/` folder for the source.

## Examples

The `examples/` folder contains worked examples:

- `flaky-component`: a simple two-port component.
- `flaky-with-delay`: a simple two-port component with subcomponents.
- `scrambler`: a component that registers a vector of subcomponents.
- `sim-pipe`: a flow-controlled pipeline.
- `sim-restaurant`: a fast food restaurant model used to explore staffing
  profitability.
- `sim-ring`: a device comprising a ring of nodes.
- `sim-fabric`: a device comprising a rectangular fabric.

### Example

The engine is created as a mutable object `engine`:

```rust,no_run
# use gwr_engine::engine::Engine;
# fn main() {
let mut engine = Engine::default();
# }
```

### Clocks

See the [`time`] module documentation for details on creating and using clocks.

### Spawner

A new asynchronous process is created using the `spawner` from the engine. For
example, creating a new process can be done with:

```rust,no_run
use gwr_engine::engine::Engine;
fn main() {
  let mut engine = Engine::default();
  let clock = engine.default_clock();
  let spawner = engine.spawner();
  spawner.spawn(async move {
    for i in 0..10 {
      clock.wait_ticks(1).await;
      println!("Waiting {i}");
    }
    Ok(())
  });
}
```

<!-- prettier-ignore-start -->

> [!Note]
> The `Engine` makes no guarantees about the order in which tasks are
> evaluated within the same clock tick.

<!-- prettier-ignore-end -->

<!-- ANCHOR_END: overview -->

## Simple Application

A very simple application connects a source to a sink and then runs the engine:

```rust
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::{connect_port, option_box_repeat};
use gwr_engine::engine::Engine;
use gwr_engine::run_simulation;

let mut engine = Engine::default();
let clock = engine.default_clock();
let mut source = Source::new_and_register(&engine, engine.top(), "source", option_box_repeat!(0x123 ; 10));
let sink = Sink::new_and_register(&engine, &clock, engine.top(), "sink");
connect_port!(source, tx => sink, rx)
    .expect("should be able to connect `Source` to `Sink`");
run_simulation!(engine);
assert_eq!(sink.num_sunk(), 10);
```

[`time`]: src/time/mod.rs
[components]: ../gwr-components/README.md
