<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# gwr-protocols

<!-- ANCHOR: intro -->

Protocols describe packet/frame types used in simulations. The `gwr_protocols`
library currently exposes a packet-construction helper, `build_packet_type!`; it
does not yet provide a catalog of complete protocol implementations or an
executable.

<!-- ANCHOR_END: intro -->

## Before running

Run commands from the GWR workspace root (the directory containing the top-level
`Cargo.toml`). In an Omoikane checkout this is `gwr/`, alongside `workspace/`.
Initialize submodules and install the build dependencies in the
[root README](../README.md#using-gwr-packages). Rustup selects the pinned Rust
toolchain. Development tools and nightly formatting are covered by
[Developing GWR Packages](../README.md#developing-gwr-packages).

## Inspect and build

```sh
cargo check -p gwr-protocols
cargo doc -p gwr-protocols --no-deps --open
```

The first command checks the library. The second generates and opens its API
documentation; neither runs a simulation or writes a packet capture.

## Packet helper contract

[build_packet_type!](src/packet.rs) generates a packet configuration struct, a
packet struct, field accessors and setters, payload storage, byte accounting,
formatting, identity and simulation-object implementations. Each declared field
supplies its Rust type, encoded bit width and scale; getters and setters apply
the declared shift/mask. The generated constructor records object creation with
`gwr-track`.

This macro is a building block for a surrounding protocol implementation. The
caller supplies its protocol header-byte constant, packet-kind enum and variant,
and the additional names/methods referenced by the expansion, including the
entity and tracking interfaces. Read the expansion before integrating it; the
repository currently contains no complete consumer example of this macro.

For existing runnable frame models, inspect
[gwr-models](../gwr-models/README.md#frames), including `MemoryAccess` and
`EthernetFrame`. To learn how objects move through typed ports, start with
[flaky-component](../examples/flaky-component/README.md).

## Recipe verification

Commands in this document are source-reviewed. They have not been smoke-run as
part of this documentation adoption; build, GUI and workload outcomes remain
unverified. Historical observations above retain their original scope.
