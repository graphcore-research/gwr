<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Resources

`Resource`s are available to model shared resources that have a limited
capacity.

The `gwr_resources` library provides a collection of shared resource primitives
to be used when building simulations. These are not intended to include or model
Graphcore IP.

## Example

An example of this is the [flow controlled pipeline] where a `Resource` is used
to model the credit within the pipeline. Credit is acquired with a `request()`
call and granted with the `release()` call:

```rust,no_run
use gwr_engine::engine::Engine;
use gwr_resources::Resource;

fn main() {
  let mut engine = Engine::default();
  let spawner = engine.spawner();
  let clock = engine.clock_ghz(1.0);

  let resource = Resource::new(1);
  // Need a clone for the credit grant process to use.
  let grant = resource.clone();

  // Request credit
  spawner.spawn(async move {
    for i in 0..10 {
        resource.request().await;
        println!("Credit granted {i}");
    }
    Ok(())
  });

  // Release credit
  spawner.spawn(async move {
    for _ in 0..10 {
        clock.wait_ticks(1).await;
        grant.release();
    }
    Ok(())
  });
}
```

[flow controlled pipeline]: ../models/chapter.md
