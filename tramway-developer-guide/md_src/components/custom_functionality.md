<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Implement Custom Functionality

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
# use tramway_engine::traits::{Runnable, SimObject};
# use tramway_engine::types::SimResult;
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

The **examples/flaky-with_delay** gives an example of a component that uses
custom `run()` functionality.

## Default Functionality

If the new component does not need to have any custom behaviour and is simply
connecting a collection of sub-components then it can implement just use the
default `Runnable` provided by the library with a `derive` statement.

```rust,no_run
# use async_trait::async_trait;
# use std::marker::PhantomData;
# use tramway_engine::traits::SimObject;
# use tramway_model_builder::Runnable;
# use tramway_engine::types::SimResult;
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

[components]: ../components/chapter.md
