<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Create a Struct

The first thing to define when creating a component is to create the `struct`s
that define the component.

All components should contain an `Entity` which is used to configure the logging
and also to give a unique location within the model hierarchy. The `Entity` will
be wrapped in [`std::rc::Rc`] so that it can be shared.

```rust,no_run
# use std::marker::PhantomData;
# use std::rc::Rc;
# use tramway_engine::traits::SimObject;
# use tramway_model_builder::EntityDisplay;
# use tramway_track::entity::Entity;
# #[allow(dead_code)]

#[derive(EntityDisplay)]
struct MyComponent<T>
where
    T: SimObject
{
    pub entity: Rc<Entity>,

    // Any component-specific state
#   phantom: PhantomData<T>
}
# fn main() {}
```

[`std::rc::Rc`]: https://doc.rust-lang.org/std/rc/struct.Rc.html
