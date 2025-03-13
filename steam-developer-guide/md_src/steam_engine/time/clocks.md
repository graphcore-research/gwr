<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Clocks

`Clock`s are used to control the time within a STEAM simulation. The [engine]
supports any number of clocks running at different frequencies.

## Creating a Clock

A clock runs at a frequency and can be created with the either the [engine]'s
`clock_ghz()` or `clock_mhz()` functions.

The following two clocks are equivalent:

```rust,no_run
# use steam_engine::engine::Engine;
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
# use steam_engine::engine::Engine;
# fn main() {
# let mut engine = Engine::default();
# let spawner = engine.spawner.clone();
let clock = engine.clock_ghz(1.0);
# spawner.spawn(async move {
clock.wait_ticks(1).await;
println!("Time now {:.2}", clock.time_now_ns());
# Ok(())
#  });
# }
```

{{#include ../../links_depth2.md}}
