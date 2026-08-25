<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-timetable

`gwr-timetable` defines workload graphs that can be validated against and run on
a `gwr-platform` machine description.

It also provides a front-end utility for running timetables:

```sh
cargo run --bin gwr-timetable -- \
  --platform gwr-platform/examples/platform.yaml \
  --timetable gwr-timetable/examples/small.yaml \
  --stdout --stdout-level debug
```
