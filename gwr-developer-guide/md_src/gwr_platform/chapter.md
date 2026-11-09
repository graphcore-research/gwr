<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# GWR Platform

{{#include ../../../gwr-platform/README.md:overview}}

{{#include ../../../gwr-platform/README.md:compute_engines}}

The
[`gwr-use-cases` package](https://github.com/graphcore-research/gwr/tree/main/gwr-use-cases)
owns machine and workload experiments such as the analytic GB200 reference and
pinned V4-Flash scenario. It records topology, address ownership, rate assumptions,
and calibration evidence separately from reusable platform machinery. The
[timetable guide][`gwr_timetable`] describes the workload graph and runner.
Inspect both machine provenance and workload assumptions when interpreting
simulation time.

[`gwr_engine`]: ../gwr_engine/chapter.md
[`gwr_models`]: ../models/chapter.md
[`gwr_timetable`]: ../applications/examples.md#gwr-timetable
[`Platform`]: ../gwr_platform/chapter.md

[gwr-use-cases]: https://github.com/graphcore-research/gwr/tree/main/gwr-use-cases
