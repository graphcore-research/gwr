<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-platform

<!-- ANCHOR: overview -->

`gwr_platform` is the library used to define and build execution platforms from
configuration files.

A platform in GWR is the structural description of the machine that a workload
runs on. It brings together named processing elements, memories, caches,
fabrics, and the connections between them into one validated object.

## What It Provides

The `gwr_platform` library provides:

- YAML configuration file support.
- build functions that construct memories, processing elements, caches, and
  fabrics via the `gwr_platform::builder` module.
- connection functions that wire a platform together via the
  `gwr_platform::connect` module.

## A Simple Platform

An example of a simple platform could include:

- a processing element
- an L1 cache
- a backing memory
- a memory map describing what memory the PE can address

For example:

```rust
# use gwr_engine::engine::Engine;
# use gwr_platform::Platform;
# fn main() {
# let mut engine = Engine::default();
# let clock = engine.default_clock();
# let _ = Platform::from_string(&engine, &clock, "
memory_maps:
  - name: pe_memory_map
    devices:
      - name: mem0

processing_elements:
  - name: pe0
    memory_map: pe_memory_map
    config:
      lsu_access_bytes: 32
      sram_bytes: 64KiB

caches:
  - name: l1_0
    memory_map: pe_memory_map
    config:
      bw_bytes_per_cycle: 32
      line_size_bytes: 32
      delay_ticks: 4

memories:
  - name: mem0
    kind: ddr
    base_address: 0x1_0000_0000
    capacity_bytes: 1GiB
    delay_ticks: 40

connections:
  - connect:
      - pe.pe0
      - cache.l1_0.dev
  - connect:
      - cache.l1_0.mem
      - mem.mem0
# ").unwrap_or_else(|err| panic!("failed to validate: {err}"));
# }
```

## Example

Load a platform from YAML and inspect the resulting structure:

```rust,no_run
use std::path::Path;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_platform::Platform;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::default();
    let clock = engine.default_clock();

    let platform = Rc::new(Platform::from_file(
        &engine,
        &clock,
        Path::new("gwr-platform/examples/simple_pe_cache_mem.yaml"),
    )?);

    println!("Platform has {} processing elements", platform.num_pes());
    println!("{platform}");
    Ok(())
}
```

<!-- ANCHOR_END: overview -->

[`Platform`]: ./src/lib.rs
[`gwr_engine`]: ../gwr-engine/README.md
[`gwr_models`]: ../gwr-models/README.md
[`gwr_timetable`]: ../gwr-timetable/src/lib.rs
