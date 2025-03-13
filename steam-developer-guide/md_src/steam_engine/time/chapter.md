<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Time-based Simulation

STEAM-based simulations can be run as purely event driven (where one event
triggers one or more other events) or the use of [clocks] can be introduced to
model time. The combination of both is the most common.

The [engine] manages the [clocks]. A simple example of a component that uses the
clock is the [`rate_limiter`] which models the amount of time it takes for
objects to pass through it.

{{#include ../../links_depth2.md}}
