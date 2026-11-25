<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Examples

Here is a collection of examples written using GWR.

## Abstract Examples

### Flaky Component

This shows a minimal runnable simulation with custom [component] behavior.

How to run it:

```bash
cargo run --release --bin flaky-component -- --seed 1 --drop 0.5 --num-packets 1000
```

Try varying `--drop` and comparing the number of packets received.

### Flaky with Delay

This shows a component using explicit time delays, buffering, and custom
functionality.

As with all the examples, you can determine the command-line arguments by
running with `--help`:

```bash
cargo run --release --bin flaky-with-delay -- --help
```

Try varying the delay as well as the drop rate.

### Scrambler

The scrambler shows how component can have dynamic port connections at runtime
depending on command-line arguments.

Compare running:

```bash
cargo run --release --bin scrambler
```

against

```bash
cargo run --release --bin scrambler -- -s
```

## System Exploration Examples

These examples show GWR being used to build larger systems that you might find
in silicon devices.

<!-- prettier-ignore-start -->

> [!Tip]
> Most of these larger simulations support `--progress` to print a
> progress bar if you are running them directly (not using `gwr-terminus`).

<!-- prettier-ignore-end -->

### Sim Pipe

This simulation shows how you can model a credit-controlled pipeline in order to
understand throughput, buffering, latency, and backpressure.

Try running it and varying the data and credit delays and the size of the data
buffer which controls the number of credits that can be issued:

```bash
cargo run --release --bin sim-pipe -- --stdout --pipe-data-delay 10 --pipe-buffer-entries 10 --pipe-credit-delay 10
```

If you now vary the size of the pipe buffer or the delays you will see the
impact on pipeline throughput.

### Sim Ring

The ring-based interconnect simulation shows how the arbitration can cause such
an architecture to deadlock.

The Terminus recipe can be used when you want to sweep ring size, arbitration
priority, buffer sizes, or trace settings without rebuilding a long command by
hand.

```bash
cargo run --release --bin terminus -- run --recipe examples/sim-ring/recipes/explore_ring_priorities.yaml
```

You will notice how having a fair priority for ring traffic causes deadlock and
it is essential to give priority to traffic in the ring to prevent this.

### Sim Fabric

This simulation demonstrates how GWR can be used to build models at different
levels of abstraction.

You can use the Terminus recipes to explore the impact of the level of
abstraction modelled, which you will see produce quite different performance
numbers:

```bash
cargo run --release --bin terminus -- run --recipe examples/sim-fabric/recipes/explore_model.yaml
```

You can show that with a different traffic pattern the two models do produce
very similar results by running the above command again and adding:

```bash
--ARGS "--traffic-pattern all-to-one"
```

There is also a recipe to show how to explore the impact of the fabric data
ticks per hop:

```bash
cargo run --release --bin terminus -- run --recipe examples/sim-fabric/recipes/explore_ticks_per_hop.yaml
```

### GWR Timetable

`gwr-timetable` runs a workload defined as a graph of tensors and compute tasks
on the processing elements and memory described by a platform file. From the GWR
repository root, run the example that adds two tensors and multiplies the result
by a third.

```bash
cargo run --release --bin gwr-timetable -- \
  --platform gwr-timetable/examples/compute_region_platform.yaml \
  --timetable gwr-timetable/examples/add_mul.yaml --stdout
```

Add `--check` to validate the workload without running it. Use `--explain` to
also see tensor bindings, PE placement, tile sizes, SRAM requirements, and
planned memory traffic.

[component]: ../components/chapter.md
