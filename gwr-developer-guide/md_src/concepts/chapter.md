<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Concepts

At the core, GWR lets you describe:

- Tasks that model independent activities.
- Data objects that represent information or physical items moving between
  tasks.
- Events that wake tasks up.
- Clocks that advance simulated time.
- State such as queues and data stores that create contention.
- Metrics and traces that give visibility of what happens in the model.

## Core Building Blocks

### Tasks

Tasks are the concurrent parts of the model implemented as async functions. They
wait for data, time, or a signal, then perform some action or update state which
may trigger other tasks.

### Data

Simulation objects model data that is acted on by tasks.

### Events

Events provide the wake-up mechanism between tasks. They are useful for
notifications such as "task complete", "data is ready", "time reached" or "space
is available".

### Clocks

Clocks make modelling time explicit as tasks choose when they want to wait for
time to advance a number of clock ticks to represent actions that take time.

### State

Queues and data stores are examples of state that handle data that the tasks act
on.

### Metrics

A model can emit log messages and trace data to help inspect why a result
occurred, not just what the final totals were. These can either be written
textually to console, a space efficient binary format, or inspected with tools
like Perfetto.
