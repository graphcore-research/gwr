<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Applications

This chapter gives an overview of how to write a top-level application.

## Create the Engine

The first thing to do is to create a simulation [`Engine`]:

```rust,no_run
# use steam_engine::engine::Engine;
# fn main() {
# #[allow(unused_variables, unused_mut)]
let mut engine = Engine::default();
# }
```

The engine provides the top-level entity for the simulation that must be used as
the parent to top-level components.

## Instantiate Components

Then simulation components can be created. An example of a very basic simulation
is to create a data [`Source`] and [`Sink`].

In this case the [`Source`] is configured to emit the value `0x123` ten times:

```rust,no_run
# use steam_components::source::Source;
# use steam_components::sink::Sink;
# use steam_components::{connect_port, option_box_repeat};
# use steam_engine::engine::Engine;
# fn main() {
# let engine = Engine::default();
let source = Source::new_and_register(&engine, engine.top(), "source", option_box_repeat!(0x123 ; 10))
    .expect("should be able to create and register `Source`");
# let sink = Sink::new_and_register(&engine, engine.top(), "sink")
#     .expect("should be able to create and register `Sink`");
# connect_port!(source, tx => sink, rx)
#     .expect("should be able to connect `Source` to `Sink`");
# }
```

```rust,no_run
# use steam_components::source::Source;
# use steam_components::sink::Sink;
# use steam_components::{connect_port, option_box_repeat};
# use steam_engine::engine::Engine;
# fn main() {
# let engine = Engine::default();
# let source = Source::new_and_register(&engine, engine.top(), "source", option_box_repeat!(0x123 ; 10))
#     .expect("should be able to create and register `Source`");
let sink = Sink::new_and_register(&engine, engine.top(), "sink")
    .expect("should be able to create and register `Sink`");
# connect_port!(source, tx => sink, rx)
#     .expect("should be able to connect `Source` to `Sink`");
# }
```

## Connect Components

Ports are connected together using the helper `connect_port!` macro. The
connections are always done in the direction of data flow `tx -> rx`.

```rust,no_run
# use steam_components::source::Source;
# use steam_components::sink::Sink;
# use steam_components::{connect_port, option_box_repeat};
# use steam_engine::engine::Engine;
# fn main() {
# let engine = Engine::default();
# let source = Source::new_and_register(&engine, engine.top(), "source", option_box_repeat!(0x123 ; 10))
#     .expect("should be able to create and register `Source`");
# let sink = Sink::new_and_register(&engine, engine.top(), "sink")
#     .expect("should be able to create and register `Sink`");
connect_port!(source, tx => sink, rx)
    .expect("should be able to connect `Source` to `Sink`");
# }
```

## Run Simulation

Now that everything has been created and connected the simulation can be run
using the `run_simulation!` macro:

```rust,no_run
# use steam_components::source::Source;
# use steam_components::sink::Sink;
# use steam_components::{connect_port, option_box_repeat};
# use steam_engine::engine::Engine;
# use steam_engine::run_simulation;
# fn main() {
# let mut engine = Engine::default();
# let source = Source::new_and_register(&engine, engine.top(), "source", option_box_repeat!(0x123 ; 10))
#     .expect("should be able to create and register `Source`");
# let sink = Sink::new_and_register(&engine, engine.top(), "sink")
#     .expect("should be able to create and register `Sink`");
# connect_port!(source, tx => sink, rx)
#     .expect("should be able to connect `Source` to `Sink`");
run_simulation!(engine);
# }
```

The `run_simulation!` spawns the components specified and then starts then runs
the engine to completion.

## Check Results

So, after the simulation has completed it is possible to check that the [`Sink`]
has received all the expected data.

```rust,no_run
# use steam_components::source::Source;
# use steam_components::sink::Sink;
# use steam_components::{connect_port, option_box_repeat};
# use steam_engine::engine::Engine;
# use steam_engine::run_simulation;
# fn main() {
# let mut engine = Engine::default();
# let source = Source::new_and_register(&engine, engine.top(), "source", option_box_repeat!(0x123 ; 10))
#     .expect("should be able to create and register `Source`");
# let sink = Sink::new_and_register(&engine, engine.top(), "sink")
#     .expect("should be able to create and register `Sink`");
# connect_port!(source, tx => sink, rx)
#     .expect("should be able to connect `Source` to `Sink`");
# run_simulation!(engine);
assert_eq!(sink.num_sunk(), 10);
# }
```

## Full Source

The entire example (including the `use` statements that are required to pull in
the dependencies) looks like this:

```rust,no_run
use steam_components::source::Source;
use steam_components::sink::Sink;
use steam_components::{connect_port, option_box_repeat};
use steam_engine::engine::Engine;
use steam_engine::run_simulation;

fn main() {
    let mut engine = Engine::default();
    let source = Source::new_and_register(&engine, engine.top(), "source", option_box_repeat!(0x123 ; 10))
        .expect("should be able to create and register `Source`");
    let sink = Sink::new_and_register(&engine, engine.top(), "sink")
        .expect("should be able to create and register `Sink`");
    connect_port!(source, tx => sink, rx)
        .expect("should be able to connect `Source` to `Sink`");
    run_simulation!(engine);
    assert_eq!(sink.num_sunk(), 10);
}
```

[`Engine`]: ../steam_engine/chapter.md
[`Sink`]: ../components/steam_components.md#sink
[`Source`]: ../components/steam_components.md#source
