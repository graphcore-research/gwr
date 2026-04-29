<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Beyond Silicon

The `sim-restaurant` example shows that GWR is not limited to silicon or packet
models. It uses the same engine to explore a restaurant as a system of queues,
workers, time delays, and business outcomes.

## Objective

To see how the GWR core can be used to model many types of systems beyond
silicon. This example shows how to run a staffing sweep to identify what
combination results in the most profitable restaurant.

## Run The Simulation

```bash
cargo run --bin sim-restaurant -- --min-till-staff 1 --max-till-staff 2 --min-kitchen-staff 1 --max-kitchen-staff 4 --top-results 4
```

The above command should produce output that looks like:

```text
Restaurant demand plan: 2642 customers from 07:00 to 22:00 (15.0 hours, seed 7).

Till Kitchen  Served   Balked   GaveUp    Revenue      Costs    Profit  Finish h  Max Queue
   1       4    1040     1375      225   13122.40    5251.01   7871.39     15.44   13/24
   2       4    1029     1357      254   13136.50    5495.15   7641.35     15.44   13/24
   1       3     773     1475      391    9880.10    4006.90   5873.20     15.52   12/24
   2       3     781     1452      405    9886.20    4252.65   5633.55     15.55   12/24
```

### Explore Using a TUI

Take a look at which staffing mix maximises profit, where queues build up, and
whether the till or kitchen is the real bottleneck. There is a command-line TUI
that allows you to inspect the details of what happens during a simulation of a
given staffing configuration:

```bash
cargo run --bin sim-restaurant-tui -- --till-staff 2 --kitchen-staff 5
```

## Mapping Restaurant Ideas To GWR

This model uses the same building blocks as a silicon simulation:

- Customers are modelled as async tasks that trigger time-based events.
- Till workers and kitchen workers are concurrent async tasks.
- Queues are explicit shared state.
- Service completion is modeled through events.
- Profitability is derived from measurable end-of-run metrics.

## Explore The Problem Space

Change one pressure point and rerun:

- Increase `--max-till-staff` to see whether the till is the bottleneck.
- Increase `--max-kitchen-staff` to see whether food preparation is the
  bottleneck.
- Reduce `--join-base-probability` or raise `--join-queue-sensitivity` to model
  less patient customers.
