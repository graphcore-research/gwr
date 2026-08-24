<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# gwr-spotter

<!-- ANCHOR: overview -->

`gwr-spotter` is a terminal user interface for viewing textual logs and Cap'n
Proto binary traces produced by [gwr-track].

## Launching

Run the following commands from the workspace root. Exactly one of `--log` or
`--bin` must be provided.

To open a binary trace:

```bash
cargo run --release --bin gwr-spotter -- --bin trace.bin
```

To open a textual log:

```bash
cargo run --release --bin gwr-spotter -- --log trace.log
```

With the default `perfetto` feature enabled, a binary trace can instead be
converted to a Perfetto trace:

```bash
cargo run --release --bin gwr-spotter -- --bin trace.bin --perfetto trace.pftrace
```

This writes `trace.pftrace` and exits without starting the interactive viewer.

## Commands

Press `?` to display the in-application help and any key to return to the trace.
Press `q` or `Ctrl-C` to quit.

## Frontend

While running interactively, `gwr-spotter` automatically starts a loopback HTTP
API at `127.0.0.1:8000`. The web frontend is served separately; see the
[frontend documentation] for setup instructions.

This feature can be disabled by passing the `--no-server` argument. This
prevents `gwr-spotter` from attempting to bind to port 8000.

<!-- ANCHOR_END: overview -->

### Views

Note that there are a number of different views of the model that are available
within the frontend. The default is a sunburst view which shows the hierarchy.
Other views can be selected using the menu on the left of the page.

[frontend documentation]: frontend/README.md
[gwr-track]: ../gwr-track/README.md
