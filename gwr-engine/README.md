<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# gwr-engine

<!-- ANCHOR: overview -->

`gwr_engine` is a single-threaded asynchronous simulation engine designed to run
models of asynchronous simulation [components].

### Example

The engine is created as a mutable object `engine`:

```rust,no_run
# use gwr_engine::engine::Engine;
# fn main() {
# #[allow(unused_variables, unused_mut)]
let mut engine = Engine::default();
# }
```

### Clocks

The engine is responsible for managing the clocks. A user can either get the
default [clock] if they are not concerned about the actual frequency that it
runs at:

The engine is created as a mutable object `engine`:

```rust,no_run
# use gwr_engine::engine::Engine;
# fn main() {
let mut engine = Engine::default();
# #[allow(unused_variables)]
let clock = engine.default_clock();
# }
```

If a well defined clock frequency is required then the engine provides access
the ability to create different clocks:

```rust,no_run
# use gwr_engine::engine::Engine;
# fn main() {
let mut engine = Engine::default();
# #[allow(unused_variables)]
let clock_1ghz = engine.clock_ghz(1.0);
# #[allow(unused_variables)]
let clock_10mhz = engine.clock_mhz(10.0);
# }
```

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
      clock.wait_ticks(1);
      println!("Waiting {i}");
    }
    Ok(())
  });
}
```

<!-- ANCHOR_END: overview -->

[clock]: ../gwr-developer-guide/md_src/gwr_engine/time/clocks.md
[components]: ../gwr-components/README.md
