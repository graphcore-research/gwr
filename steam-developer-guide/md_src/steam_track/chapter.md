<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# STEAM Track

`steam_track` is the logging library used by the STEAM [engine] and all
components. It can be used to generate either text-based or [Cap'n Proto]-based
binary log files.

## Entities

`steam_track` provides the [`Entity`] struct. It is designed to represent a
unique simulation [`Entity`]/[component] which exists within the simulation
hierarchy.

An [`Entity`] will have a unique location within the simulation hierarchy and
are each assigned a unique [`Id`]. For example:

```bash
top::processor0::cpu102::memory
```

Tracing can be configured globally or enabled/disabled depending on regular
expressions matching entity names.

## IDs

Each [`Entity`] will have a unique 64-bit [`Id`]. This ID is used throughout the
log/bin files in order to identify the originator of messages and to reduce the
size of the files.

`Id`s are also assigned to all packets. Packets do not contain an [`Entity`]
because they flow through the simulation and so their location within the
simulation changes. However, the logging of packets is controlled by the
[`Entity`] that creates the packet.

## Macros

The library provides a number of macros that provide the logging functionlity
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

And then there are macros that map to simulation events:

- `create!` - used when a new [`Id`] is created.
- `destroy!` - used when a [`Id`] is destroyed.
- `connect!` - used when an [`Entity`] is connected to another [`Entity`].
- `enter!` - used when an [`Entity`] enters another [`Entity`]. For example, a
  packet enters a buffer.
- `exit!` - used when an [`Entity`] leaves another [`Entity`]. For example a
  packet leaves a pipeline.

[Cap'n Proto]: https://capnproto.org
[component]: ../components/chapter.md
[engine]: ../steam_engine/chapter.md
[`Entity`]: ../steam_track/chapter.md#entities
[`Id`]: ../steam_track/chapter.md#ids
