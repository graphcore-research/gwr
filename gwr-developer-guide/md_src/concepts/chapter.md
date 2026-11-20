<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Concepts

At the core, GWR lets you describe:

- Tasks that model independent activities.
- Data "packets" that represent information or physical items moving between
  tasks.
- Events that wake components up.
- Clocks that synchronise components.
- State such as queues and data stores that create contention.
- Metrics and traces that give visibility of what happens in the model.

## Core Building Blocks

### Components

Components are the concurrent parts of the model, with their behaviour
implemented as async functions. They wait for a packet, time, or a signal, then
perform some action or update state which may trigger other components.

### Events

Events provide the wake-up mechanism between tasks. They are useful for
notifications such as "task complete", "data is ready", "time reached" or "space
is available".

### Clocks

Clocks allow synchronisation between components. A component can hold a
reference to a clock, and use it to wait a specified number of ticks before
being automatically woken to continue running. Any number of components can hold
references to the same clock, and any number of clocks can be defined in a
single engine.

### Models

Models are combinations of components connected together to form a simulation of
the behaviour of an electronic system, such as a ring of ethernet switches or a
data cache.

### Platforms

GWR is often used to model machine learning training and inference systems.
Since these uses have high complexity and size, the process of constructing
simulations of systems to run ML workloads is abstracted by platforms, allowing
the definition and validation of complex systems via .yaml files.

### Timetables

Timetables allow a user to define ML workloads. These can then be simulated on
platforms.

### Metrics

A model can emit log messages and trace data to help inspect why a result
occurred, not just what the final totals were. These can either be written
textually to console, a space efficient binary format, or inspected with tools
like Perfetto.
