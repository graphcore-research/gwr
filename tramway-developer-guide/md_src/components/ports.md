<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Ports

A component will have a number of ports which provide its interfaces to other
components.

## Output / Input

Ports can either be "output" or "input". A connection must always be made
between one output and one input port.

## Data Types

The type of the port is specialised by the data type that it carries. Ports have
to be of the same type to be connected together.

## Component Ports

Components provide functions that allow the connection of their ports. Ports can
either be connected directly to a component or to a subcomponent. It is
therefore up to the component writer to provide the relevant functions and
connect the ports as required.

Port connection functions take two forms - those that take arrays indices and
those that don't. Each function will have a unique name depending on the port
name and the direction of data flow. Some examples are provided below.

The [Flaky component] also provides a example of a custom component.

### Input Ports

The function naming is critical. The method for an input port will return a
shared reference to a shared state that is then passed to the output to complete
the connection.

Here are a few examples:

A component with a single input port called `rx` will have:

```rust,no_run
# use std::marker::PhantomData;
# use tramway_engine::port::PortStateResult;
# use tramway_engine::traits::SimObject;
# #[allow(dead_code)]
# struct TestBlock<T> { phantom: PhantomData<T> }
# impl<T: SimObject> TestBlock<T> {
# #[allow(dead_code)]
pub fn port_rx(&self) -> PortStateResult<T>
# { todo!() }
# }
# fn main() {}
```

A component with an array of input ports called `in` will have:

```rust,no_run
# use std::marker::PhantomData;
# use tramway_engine::port::PortStateResult;
# use tramway_engine::traits::SimObject;
# #[allow(dead_code)]
# struct TestBlock<T> { phantom: PhantomData<T> }
# impl<T: SimObject> TestBlock<T> {
# #[allow(dead_code, unused_variables)]
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
# use tramway_engine::port::PortStateResult;
# use tramway_engine::traits::SimObject;
# use tramway_engine::types::SimResult;
# #[allow(dead_code)]
# struct TestBlock<T> { phantom: PhantomData<T> }
# impl<T: SimObject> TestBlock<T> {
# #[allow(dead_code, unused_variables)]
pub fn connect_port_tx(&self, port_state: PortStateResult<T>) -> SimResult
# { todo!() }
# }
# fn main() {}
```

A component with an array of output ports called `out` will have:

```rust,no_run
# use std::marker::PhantomData;
# use tramway_engine::port::PortStateResult;
# use tramway_engine::traits::SimObject;
# use tramway_engine::types::SimResult;
# #[allow(dead_code)]
# struct TestBlock<T> { phantom: PhantomData<T> }
# impl<T: SimObject> TestBlock<T> {
# #[allow(dead_code, unused_variables)]
pub fn connect_port_out_i(&self, i: usize, port_state: PortStateResult<T>) -> SimResult
# { todo!() }
# }
# fn main() {}
```

# Connecting Ports

Connections are always made in the direction of flow of data (`tx` -> `rx`). For
example:

```rust,no_run
# use tramway_components::sink::Sink;
# use tramway_components::source::Source;
# use tramway_components::{connect_port, option_box_repeat};
# use tramway_engine::engine::Engine;
# fn main() {
# let num_puts = 10;
# let mut engine = Engine::default();
let mut source = Source::new_and_register(&engine, engine.top(), "source", option_box_repeat!(0x123 ; num_puts))
    .expect("should be able to create and register `Source`");
let sink = Sink::new_and_register(&engine, engine.top(), "sink")
    .expect("should be able to create and register `Sink`");
connect_port!(source, tx => sink, rx)
    .expect("should be able to connect `Source` to `Sink`");
}
```

## Errors

If attempting to connect ports that don't exist on the source/dest components
then there will be a compile error.

```rust,compile_fail
# use tramway_components::sink::Sink;
# use tramway_components::source::Source;
# use tramway_components::{connect_port, option_box_repeat};
# use tramway_engine::engine::Engine;
# fn main() {
# let num_puts = 10;
# let mut engine = Engine::default();
let mut source = Source::new_and_register(&engine, engine.top(), "source", option_box_repeat!(0x123 ; num_puts))
    .expect("should be able to create and register `Source`");
let sink = Sink::new_and_register(&engine, engine.top(), "sink")
    .expect("should be able to create and register `Sink`");
connect_port!(source, tx => sink, invalid)
    .expect("should be able to connect `Source` to `Sink`");
# }
```

[Flaky component]: ../components/writing_a_component.md
