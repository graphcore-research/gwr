<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# gwr-track

<!-- ANCHOR: overview -->

`gwr_track` is the logging library used by the GWR [engine] and all components.
It can be used to generate either text-based or [Cap'n Proto]-based binary log
files.

## Entities

`gwr_track` provides the `Entity` struct. It is designed to represent a unique
instance of a [component] in the simulation hierarchy.

An `Entity` will have a unique location within the simulation hierarchy and is
assigned a unique [`Id`]. For example:

```bash
top::processor0::cpu102::memory
```

Tracing can be configured globally or enabled/disabled depending on regular
expressions matching entity names.

## EntityMonitor

The `EntityMonitor` allows the user to create helper structs that can monitor an
`Entity` and emit useful statistics through `track_value()` calls.

## Packet

Packets are given unique `Id`s like `Entities` so that their lifecycle can be
tracked across the simulation.

## IDs

Each [`Entity`] will have a unique 64-bit [`Id`]. This ID is used throughout the
log/bin files in order to identify the originator of messages.

`Id`s are also assigned to track packets. Packets do not contain an [`Entity`]
because they flow through the simulation and so their location within the
simulation changes. However, the logging of packets is controlled by the
[`Entity`] that creates the object.

## Macros

The library provides a number of macros that provide the logging functionality
with a minimal run-time overhead when not enabled.

There are the macros that map to log messages of the specified level:

- `trace!` - a message that will only be emitted if log level is `Trace`.
- `debug!` - a message that will only be emitted if log level is `Debug` or
  above.
- `info!` - a message that will only be emitted if log level is `Info` or above.
- `warn!` - a message that will only be emitted if log level is `Warn` or above.
- `error!` - a message that will only be emitted if log level is `Error` or
  above.

**Note:** the logging level is controlled globally with the ability to configure
it at the level of any [`Entity`] within the simulation hierarchy.

[Cap'n Proto]: https://capnproto.org
[`Entity`]: #entities
[`Id`]: #ids

<!-- ANCHOR_END: overview -->

[component]: ../gwr-components/README.md
[engine]: ../gwr-engine/README.md
