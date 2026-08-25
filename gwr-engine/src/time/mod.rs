// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

/*!
Modules that model time within the simulations.

<!-- ANCHOR: clock_overview -->

Clocks are used to control time within a GWR simulation. The [engine] supports any
number of clocks running at different frequencies.

Each clock represents a clock domain. This lets one simulation contain blocks
that operate at different frequencies without converting everything into one
global tick rate. For example, a platform can model a 1GHz block and a 2GHz
block with separate clocks; one tick on the 1GHz clock and two ticks on the 2GHz
clock both represent `1.0ns`, but each clock domain keeps its own local tick
count.

The engine schedules clock waits by absolute simulation time and then by clock
phase. Tasks remain event-driven: they only wake when their awaited event, port,
or clock wait is ready.

If two clock ticks resolve to exactly the same time and phase the [engine] does
not provide any guarantees about which events will be evaluated first.

## Default Clock

The [engine] is responsible for managing clocks. Use the default clock when the
frequency does not matter (the default is currently 1GHz, but that may change):

```rust,no_run
# use gwr_engine::engine::Engine;
# fn main() {
let mut engine = Engine::default();
let clock = engine.default_clock();
# }
```

## Creating a Clock

When a well-defined clock frequency is required, create clocks explicitly.
A non-default clock runs at a user-specified frequency and can be created
with the [engine]'s helper functions.

The following two clocks are equivalent:

```rust,no_run
# use gwr_engine::engine::Engine;
# fn main() {
let mut engine = Engine::default();
let clock_a = engine.clock_ghz(1.0);
let clock_b = engine.clock_mhz(1000.0);
# }
```

## Advancing Time

Time is advanced by waiting an integer number of ticks on a clock. In the
snippet below the `println!` will be called when the time has advanced to
`1.0ns`.

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

## Clock Phases

Clock time is represented using the `ClockTick` type, made up of a tick count and a phase
within that tick. Phases provide deterministic ordering inside a tick.

The two standard phases are:

- `phase::BEGIN`: the start of a tick. `wait_ticks(...)` waits resume in this
  phase.
- `phase::END`: the end of a tick. Components can wait for this phase when they
  need all same-tick releases or bookkeeping to happen before starting new
  activity.

Custom `u32` phase values can be used between `phase::BEGIN` and `phase::END`
when a model needs additional deterministic ordering points. A task can use
`wait_phase(phase)` to move later within the current tick, or
`next_tick_and_phase(phase)` to wait until a specific phase in the next tick.

For example, certain models use `phase::END` before starting new activities
in order to guarantee that any completing activities have been processed first:

```rust,no_run
# use gwr_engine::engine::Engine;
# use gwr_engine::time::clock::phase;
# fn main() {
# let mut engine = Engine::default();
let clock = engine.clock_ghz(1.0);
# let spawner = engine.spawner();
# spawner.spawn(async move {
clock.wait_phase(phase::END).await;
// Begin activity that should be ordered after same-tick completions.
# Ok(())
# });
# }
```

## Background Tasks

By default a simulation will run until all events have completed. However,
sometimes it is useful to create a monitor task like a progress bar that just
needs to run as long as the rest of the simulation.

In order to do this the `wait_ticks_or_exit` function can be called. This lets
the [engine] know that it does not have to keep running if this is the only thread
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

<!-- ANCHOR_END: clock_overview -->

[engine]: ../engine/index.html
*/

use byte_unit::{AdjustedByte, Byte, UnitType};

pub mod clock;
pub mod simtime;

// Convert a number of bytes to a binary-only unit (KiB, MiB, etc)
#[must_use]
pub fn compute_adjusted_value_and_rate(
    time_now_ns: f64,
    num_bytes: usize,
) -> (AdjustedByte, AdjustedByte) {
    let time_now_s = time_now_ns / (1000.0 * 1000.0 * 1000.0);
    let count = Byte::from_u64(num_bytes as u64).get_appropriate_unit(UnitType::Binary);
    let per_second = if time_now_s == 0.0 {
        Byte::from_f64(0.0).unwrap()
    } else {
        Byte::from_f64(num_bytes as f64 / time_now_s).unwrap()
    };
    let count_per_second = per_second.get_appropriate_unit(UnitType::Binary);
    (count, count_per_second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjusted_value_and_rate_handles_zero_and_elapsed_time() {
        let (count, rate) = compute_adjusted_value_and_rate(0.0, 1024);
        assert_eq!(count.get_value(), 1.0);
        assert_eq!(rate.get_value(), 0.0);

        let (count, rate) = compute_adjusted_value_and_rate(1_000_000_000.0, 2048);
        assert_eq!(count.get_value(), 2.0);
        assert_eq!(rate.get_value(), 2.0);
    }
}
