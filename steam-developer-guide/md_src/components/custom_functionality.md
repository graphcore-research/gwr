<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Implement Custom Functionality

The `pub async run(&self)` method is defined by all [components] that provide
custom functionality or have sub-components to start running as well.

```rust,no_run
# use std::marker::PhantomData;
# use steam_engine::traits::SimObject;
# use steam_engine::types::SimResult;
#
# struct MyComponent<T>
# where
#    T: SimObject
# {
#    phantom: PhantomData<T>
# }
impl<T: SimObject> MyComponent<T> {
    pub async fn run(&self) -> SimResult {
        // Spawn any sub-components

        // Implement custom-functionality

        // Return result - Ok unless there is an error to raise
        Ok(())
    }
}
# fn main() {}
```

The **examples/flaky-with_delay** gives an example of a component that uses
custom `run()` functionality.

[components]: ../components/chapter.md
