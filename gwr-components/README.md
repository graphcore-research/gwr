<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# gwr-components

<!-- ANCHOR: intro -->

Simulation components are the basic building blocks of any GWR model.

The GWR [`Engine`] runs components that are connected together using [`ports`].

The `gwr_components` library provides a collection of connectable component
primitives to be used when building models.

<!-- ANCHOR_END: intro -->

[`Engine`]: ../gwr-engine/README.md

## Creating New Components

<!-- ANCHOR: creating_new_components -->

Components are designed to be composable and connectable simulation blocks. When
creating a new one it is important to consider all of the following steps:

1. [Design the component]
1. [Create a struct]
1. [Add ports]
1. [Create subcomponents]
1. [Implement any custom functionality]
1. Provide default implementations for other methods.

This documentation will take you through designing a custom component that will
be used to drop a random number of objects that pass through it.

<!-- ANCHOR_END: creating_new_components -->

## Design the Component

<!-- ANCHOR: design_the_component -->

There are a number of things to consider when designing a new simulation
component. The two main aspects are

1. [What are the interfaces to other components](#component-interfaces)
1. [What functionality will it have internally](#component-functionality)

### Component Interfaces

An interface will comprise one or more [ports] and define how a component
connects to and interacts with other components.

So it is first essential to define the types of interfaces a component will have
and how many of each there will be. Then, the required ports can be created.

A port has a flow of data. The general naming convention is:

- Where data flows _in_ to a component it is a receive port (`rx`).
- Where data flows _out_ of a component it is a transmit port (`tx`).

### Component Functionality

Some components are simply collections of other components plugged together. In
most cases, however, it will be necessary to define custom functionality for the
port. This includes how the ports handle data they send/receive as well as
general activity that can be [spawned] in the [`run()`] function.

<!-- ANCHOR_END: design_the_component -->

## Create a Struct

<!-- ANCHOR: create_a_struct -->

The first thing to define when creating a component is to create the `struct`s
that define the component.

All components should contain an `Entity` which is used to configure the logging
and also to give a unique location within the model hierarchy. The `Entity` will
be wrapped in `std::rc::Rc` so that it can be shared.

```rust,no_run
# use std::marker::PhantomData;
# use std::rc::Rc;
# use gwr_engine::traits::SimObject;
# use gwr_model_builder::{EntityGet, EntityDisplay};
# use gwr_track::entity::Entity;

#[derive(EntityGet, EntityDisplay)]
struct MyComponent<T>
where
    T: SimObject
{
    entity: Rc<Entity>,

    // Any component-specific state
#   phantom: PhantomData<T>
}
# fn main() {}
```

<!-- ANCHOR_END: create_a_struct -->

## Ports

<!-- ANCHOR: ports -->

A component will have a number of ports which provide its interfaces to other
components.

### Output / Input

Ports can either be "output" or "input". A connection must always be made
between one output and one input port.

### Data Types

The type of the port is specialised by the data type that it carries. Ports have
to be of the same type to be connected together.

### Component Ports

Components provide functions that allow the connection of their ports. Ports can
either be connected directly to a component or to a subcomponent. It is
therefore up to the component writer to provide the relevant functions and
connect the ports as required.

Port connection functions take two forms - those that take arrays indices and
those that don't. Each function will have a unique name depending on the port
name and the direction of data flow. Some examples are provided below.

### Input Ports

The function naming is critical. The method for an input port will return a
shared reference to a shared state that is then passed to the output to complete
the connection.

Here are a few examples:

A component with a single input port called `rx` will have:

```rust,no_run
# use std::marker::PhantomData;
# use gwr_engine::port::PortStateResult;
# use gwr_engine::traits::SimObject;
# struct TestBlock<T> { phantom: PhantomData<T> }
# impl<T: SimObject> TestBlock<T> {
pub fn port_rx(&self) -> PortStateResult<T>
# { todo!() }
# }
# fn main() {}
```

A component with an array of input ports called `in` will have:

```rust,no_run
# use std::marker::PhantomData;
# use gwr_engine::port::PortStateResult;
# use gwr_engine::traits::SimObject;
# struct TestBlock<T> { phantom: PhantomData<T> }
# impl<T: SimObject> TestBlock<T> {
pub fn port_in_i(&self, i: usize) -> PortStateResult<T>
# { todo!() }
# }
# fn main() {}
```

### Output Ports

Output ports are connected by passing in the shared state that both sides of the
interface use. If the port is already connected then a `panic!` will be raised.

A component with a single output port called `tx` will have:

```rust,no_run
# use std::marker::PhantomData;
# use gwr_engine::port::PortStateResult;
# use gwr_engine::traits::SimObject;
# use gwr_engine::types::SimResult;
# struct TestBlock<T> { phantom: PhantomData<T> }
# impl<T: SimObject> TestBlock<T> {
pub fn connect_port_tx(&self, port_state: PortStateResult<T>) -> SimResult
# { todo!() }
# }
# fn main() {}
```

A component with an array of output ports called `out` will have:

```rust,no_run
# use std::marker::PhantomData;
# use gwr_engine::port::PortStateResult;
# use gwr_engine::traits::SimObject;
# use gwr_engine::types::SimResult;
# struct TestBlock<T> { phantom: PhantomData<T> }
# impl<T: SimObject> TestBlock<T> {
pub fn connect_port_out_i(&self, i: usize, port_state: PortStateResult<T>) -> SimResult
# { todo!() }
# }
# fn main() {}
```

### Connecting Ports

Connections are always made in the direction of flow of data (`tx` -> `rx`). For
example:

```rust,no_run
# use gwr_components::sink::Sink;
# use gwr_components::source::Source;
# use gwr_components::{connect_port, option_box_repeat};
# use gwr_engine::engine::Engine;
# fn main() {
# let num_puts = 10;
# let mut engine = Engine::default();
# let clock = engine.default_clock();
let mut source = Source::new_and_register(&engine, engine.top(), "source", option_box_repeat!(0x123 ; num_puts));
let sink = Sink::new_and_register(&engine, &clock, engine.top(), "sink");
connect_port!(source, tx => sink, rx)
    .expect("should be able to connect `Source` to `Sink`");
}
```

### Errors

If attempting to connect ports that don't exist on the source/dest components
then there will be a compile error.

```rust,compile_fail
# use gwr_components::sink::Sink;
# use gwr_components::source::Source;
# use gwr_components::{connect_port, option_box_repeat};
# use gwr_engine::engine::Engine;
# fn main() {
# let num_puts = 10;
# let mut engine = Engine::default();
# let clock = engine.default_clock();
let mut source = Source::new_and_register(&engine, engine.top(), "source", option_box_repeat!(0x123 ; num_puts));
let sink = Sink::new_and_register(&engine, &clock, engine.top(), "sink");
connect_port!(source, tx => sink, invalid)
    .expect("should be able to connect `Source` to `Sink`");
# }
```

<!-- ANCHOR_END: ports -->

## Create Subcomponents

<!-- ANCHOR: create_subcomponents -->

A component is the building block of models. It will have ports and usually
comprise subcomponents and some extra logic.

The **examples/flaky-with-delay** gives an example of a simple component that
uses an existing subcomponent.

<!-- ANCHOR_END: create_subcomponents -->

## Implement Custom Functionality

<!-- ANCHOR: implement_custom_functionality -->

Each component must implement the `Runnable` trait which allows it to be
registered with the `Engine` to ensure that it is run when the simulation
starts.

The `async run(&self)` method is defined by all [components] that provide custom
functionality.

Currently this relies on the `#[async_trait(?Send)]` support for async traits.
The `(?Send)` decoration indicating that only single-threaded support is
required.

```rust,no_run
# use async_trait::async_trait;
# use std::marker::PhantomData;
# use gwr_engine::traits::{Runnable, SimObject};
# use gwr_engine::types::SimResult;
#
# struct MyComponent<T>
# where
#    T: SimObject
# {
#    phantom: PhantomData<T>
# }
#[async_trait(?Send)]
impl<T> Runnable for MyComponent<T> where T: SimObject {
    async fn run(&self) -> SimResult {
        // Implement custom-functionality

        // Return result - Ok unless there is an error to raise
        Ok(())
    }
}
# fn main() {}
```

The **examples/flaky-with-delay** gives an example of a component that uses
custom `run()` functionality.

### Default Functionality

If the new component does not need to have any custom behaviour and is simply
connecting a collection of sub-components then it can implement just use the
default `Runnable` provided by the library with a `derive` statement.

```rust,no_run
# use async_trait::async_trait;
# use std::marker::PhantomData;
# use gwr_engine::traits::SimObject;
# use gwr_model_builder::Runnable;
# use gwr_engine::types::SimResult;
#
#[derive(Runnable)]
struct MyComponent<T>
where
   T: SimObject
{
    // Component members
#    phantom: PhantomData<T>
}
# fn main() {}
```

<!-- ANCHOR_END: implement_custom_functionality -->

<!-- ANCHOR: testing -->

## Testing

Components can be tested by connecting them into a small simulation and driving
their ports directly. For simple cases this can be done by hand with
`OutPort`/`InPort`, but most component tests need the same testbench structure:

1. Create an engine and the device under test (DUT).
1. Connect driver ports to DUT input ports.
1. Connect receiver ports to DUT output ports.
1. Run a sequence of sends, expects, delays, and no-traffic checks.

The `build_component_harness!` macro will generate the repeated testbench code.
It generates the harness `struct`, `Port`/`Step` enums, helper macros, etc.

Harnesses are usually declared inside a small test module. This keeps generated
names such as `Port`, `Step`, and the helper macros local to the harness and
avoids clashes with other harnesses in the same test file.

For example, the harness around a `Delay` component is created and used below:

```rust,no_run
mod delay_harness {
    use std::rc::Rc;

    use gwr_components::build_component_harness;
    use gwr_components::delay::Delay;
    use gwr_engine::test_helpers::start_test;

    build_component_harness! {
        harness DelayHarness<T> {
            component: delay: Rc<Delay<T>>,
            rx ports: {
                Rx<T> => rx,
            },
            tx ports: {
                Tx<T> => tx,
            },
        }
    }

    #[test]
    fn delay_forwards_values() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 5).unwrap();
        let mut harness = DelayHarness::new(engine, delay);

        harness.run_steps([
            send_rx!(10),
            expect_no_traffic!(&[Port::Tx], 4),
            expect_tx!(10),
        ]);
    }
}
```

The macro supports scalar RX/TX ports and RX/TX port arrays. Each port section
is optional, so a source-only component can define only `tx ports` and a
sink-only component can define only `rx ports`.

`Step` can be a send, expect, delay, no-traffic check, `Seq(Vec<Step<...>>)`
that runs child steps in order, or `Par(Vec<Step<...>>)` that runs child steps
concurrently and waits for all of them before moving on. The generated `seq!`
and `par!` helper macros build those recursive control structures and record
their source location, so tests can express parallel sequences on different
ports while keeping error messages tied to the call site.

The harness checks that each step is used on a compatible port; for example,
using an expect step on an RX port or a send step on a TX port will fail the
test.

Use `run_steps([Step<...>])` for fixed test sequences and
`run_step_generator(iterator)` for stateful generators that yield steps as the
test progresses.

<!-- ANCHOR_END: testing -->

[Add ports]: #ports
[components]: #gwr-components
[Create a struct]: #create-a-struct
[Create subcomponents]: #create-subcomponents
[Design the component]: #design-the-component
[Implement any custom functionality]: #implement-custom-functionality
[ports]: #ports
[spawned]: ../gwr-engine/README.md#spawner
[`run()`]: #implement-custom-functionality
[`ports`]: #ports
