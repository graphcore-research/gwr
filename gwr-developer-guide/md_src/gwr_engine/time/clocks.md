<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Clocks

`Clock`s are used to control the time within a GWR simulation. The [engine]
supports any number of clocks running at different frequencies.

## Creating a Clock

A clock runs at a frequency and can be created with the either the [engine]'s
`clock_ghz()` or `clock_mhz()` functions.

The following two clocks are equivalent:

```rust,no_run
# use gwr_engine::engine::Engine;
# fn main() {
let mut engine = Engine::default();
# #[allow(unused_variables)]
let clock_a = engine.clock_ghz(1.0);
# #[allow(unused_variables)]
let clock_b = engine.clock_mhz(1000.0);
# }
```

## Advancing Time

Time is advanced by waiting an integer number of ticks on a clock. In the
snippet below the `println!` will be called when the time has advanced to
`1.0ns`

```rust,no_run
# use gwr_engine::engine::Engine;
# fn main() {
# let mut engine = Engine::default();
# let spawner = engine.spawner();
let clock = engine.clock_ghz(1.0);
# spawner.spawn(async move {
clock.wait_ticks(1).await;
println!("Time now {:.2}", clock.time_now_ns());
# Ok(())
#  });
# }
```

## Background Tasks

By default a simulation will run until all events have completed. However,
sometimes it is useful to create a monitor task like a progress bar that just
needs to run as long as the rest of the simulation.

In order to do this the `wait_ticks_or_exit` function can be called. This lets
the engine know that it does not have to keep running if this is the only thread
of activity left. For example, the code below will start a thread of activity
that prints the current time in `ns` periodically as long as the simulation is
running:

```rust,no_run
# use gwr_engine::engine::Engine;
# fn main() {
# let mut engine = Engine::default();
# let spawner = engine.spawner();
let clock = engine.clock_ghz(1.0);
spawner.spawn(async move {
  loop {
    clock.wait_ticks_or_exit(1000).await;
    println!("Time now {:.2}", clock.time_now_ns());
  }
});
# }
```

[engine]: ../../gwr_engine/chapter.md
