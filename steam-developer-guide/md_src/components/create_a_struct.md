<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Create a Struct

The first thing to define when creating a component is to create the `struct`s
that define the component. Generally this is done using one `struct` for the
state and one top-level component which wraps up the state and can be copied and
passed around.

All components should contain an `Entity` which is used to configure the logging
and also to give a unique location within the model hierarchy. The `Entity` will
be wrapped in [`sync::Arc`].

Using the Rust `derive` attribute the component can be made into something that
can be `Clone`d and also displayed.

```rust,no_run
# use std::marker::PhantomData;
# use std::rc::Rc;
# use std::sync::Arc;
# use steam_engine::traits::SimObject;
# use steam_model_builder::EntityDisplay;
# use steam_track::entity::Entity;
# #[allow(dead_code)]
struct MyComponentState<T>
where
    T: SimObject
{
    // Any component state
# phantom: PhantomData<T>
}

#[derive(Clone, EntityDisplay)]
struct MyComponent<T>
where
    T: SimObject
{
    pub entity: Arc<Entity>,
    state: Rc<MyComponentState<T>>,
}
# fn main() {}
```

[`sync::Arc`]: https://doc.rust-lang.org/std/sync/struct.Arc.html
