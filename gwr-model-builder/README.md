<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-model-builder

Procedural derive macros that remove repeated boilerplate from GWR components
and models. This is a Rust library, with no executable or model-file generator.

| Derive          | Generated behavior                                                     |
| --------------- | ---------------------------------------------------------------------- |
| `EntityGet`     | Implements `gwr_track::entity::GetEntity` by returning `&self.entity`. |
| `EntityDisplay` | Implements `Display` by formatting `self.entity`.                      |
| `Runnable`      | Implements the default, empty `gwr_engine::traits::Runnable`.          |

Use the empty `Runnable` implementation for a composite whose subcomponents
already provide the required tasks. A component with custom asynchronous work
must implement `Runnable::run()` itself instead of deriving it.

## Before running

Run commands from the GWR workspace root (the directory containing the top-level
`Cargo.toml`). In an Omoikane checkout this is `gwr/`, alongside `workspace/`.
Initialize submodules and install the build dependencies in the
[root README](../README.md#using-gwr-packages). Rustup selects the pinned Rust
toolchain. Development tools and nightly formatting are covered by
[Developing GWR Packages](../README.md#developing-gwr-packages).

## Use in a component

The derives expect an `entity` field. The consuming crate needs the GWR engine,
tracking and model-builder crates; deriving `Runnable` also requires
`async_trait` to be in scope. This abbreviated consumer example follows the
[scrambler component](../examples/scrambler/src/lib.rs):

```rust,ignore
use std::rc::Rc;
use async_trait::async_trait;
use gwr_model_builder::{EntityDisplay, EntityGet, Runnable};
use gwr_track::entity::Entity;

#[derive(EntityGet, EntityDisplay, Runnable)]
struct Composite {
    entity: Rc<Entity>,
    // Store subcomponents here.
}
```

The derives do not create constructors, ports, subcomponents or engine
registrations. Those remain explicit in the consuming component.

## Explore and check

```sh
cargo run --release --bin scrambler
cargo test -p gwr-model-builder
cargo doc -p gwr-model-builder --no-deps --open
```

The scrambler prints its sink counts; the documentation command generates the
macro API documentation and opens it in a browser. See the
[component guide](../gwr-components/README.md) for construction and custom
tasks. The macro expansions are small enough to read directly in
[src/lib.rs](src/lib.rs).

## Recipe verification

Commands in this document are source-reviewed. They have not been smoke-run as
part of this documentation adoption; build, GUI and workload outcomes remain
unverified. Historical observations above retain their original scope.
