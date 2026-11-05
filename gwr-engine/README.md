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
      clock.wait_ticks(1)?.await;
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

### Quiescent batches and construction transactions

Aggregate models can opt into `Engine::register_quiescence`. Producers request
its cloneable handle; repeated requests coalesce. At each simulation time and
phase, the executor drains runnable tasks and simultaneous clock wakes, then
runs a snapshot of pending callbacks in ascending caller-supplied ID order.
Callbacks may wake tasks or request another round, but cannot recursively run
the engine. Tasks drain before the next snapshot; later phases form separate
batches. More than 10,000 rounds at one time/phase produces a diagnostic error.

This orders callback batches, not arbitrary detailed-port acceptance. Producer
polling remains unordered, so models must sort their own batch contents by
stable identity where order matters.

Factories can call `Engine::next_quiescence_id()` and immediately register the
returned ID. It avoids committed and staged IDs but does not reserve one.
Allocation follows construction order and is independent of trace entity IDs,
which need not be unique when tracing is disabled.

```rust
use std::cell::Cell;
use std::rc::Rc;
use gwr_engine::engine::Engine;
use gwr_engine::quiescence::QuiescenceCallback;
use gwr_engine::types::SimResult;

struct Batch(Cell<usize>);
impl QuiescenceCallback for Batch {
    fn on_quiescence(&self) -> SimResult {
        self.0.set(self.0.get() + 1);
        Ok(())
    }
}

fn main() {
let mut engine = Engine::default();
let transaction = engine.registration_transaction().unwrap();
let batch = Rc::new(Batch(Cell::new(0)));
let handle = engine.register_quiescence(10, batch.clone()).unwrap();
// Existing component constructors using engine.register(...) are also staged.
transaction.commit();
handle.request().unwrap();
handle.request().unwrap();
engine.run().unwrap();
assert_eq!(batch.0.get(), 1);
}
```

Dropping a transaction without committing discards its components, callbacks,
and callback-owned drain observers. Escaped callback handles become invalid;
staged callbacks cannot be requested. Nested transactions, execution during
construction, and runtime spawning during construction are forbidden. Spawning
panics before enqueuing work; run/step and nested transactions return errors.
The guard does not undo entity allocation or construction trace records.
Platform builders must explicitly use this guard; it is not automatic.

A domain with unfinished service, latency tails, transactions, or joins must
implement `QuiescenceCallback::check_drained`. At natural termination the engine
calls these checks and includes the callback ID with any stall diagnostic.
Observers should include blocked flow IDs, slots, dependencies, and expected
response producers; the engine cannot infer those from parked futures.

`run_until` retains its early-stop behavior: once its event waiter requests
termination, no further callback round or drain check is forced. A synchronous
callback snapshot already running finishes before tasks are polled again.
Pending callbacks survive the return; arbitrary restart of detailed tasks is not
guaranteed. Engines with no registered callbacks retain their existing batching
and termination behavior.

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
