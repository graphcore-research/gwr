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

<!-- ANCHOR: frontend_usage -->

## Frontend Usage

The `frontend` directory contains a static [D3.js] frontend for visualising and
interacting with a model loaded into `gwr-spotter` from a binary trace.

Run all commands below from the workspace root. The frontend and `gwr-spotter`
run as separate processes on the same machine.

### 1. Create a Trace

The first step is to create a binary trace by running a GWR-based simulation.
For example:

```bash
cargo run --release --bin sim-ring -- --binary --binary-file trace.bin
```

### 2. Start gwr-spotter

Start `gwr-spotter` with the binary trace:

```bash
cargo run --release --bin gwr-spotter -- --bin trace.bin
```

Keep `gwr-spotter` running. It starts a loopback data API at
`http://127.0.0.1:8000` for the frontend; this API is not itself a web page.
Serve the frontend separately in the next step.

### 3. Serve the Frontend

Serve the static frontend files on a second loopback port using Python:

```bash
python3 -m http.server 9991 --bind 127.0.0.1 --directory gwr-spotter/frontend
```

### 4. Open the Frontend

Open <http://localhost:9991> in a web browser on the same machine.

## Features

- Sunburst, force-tree, treemap, and radial tidy-tree visualisations. Sunburst
  is the default.
- Node selection that applies an ID filter in the `gwr-spotter` TUI.
- A trace-position slider showing the current line, total line count, and
  simulation time. Moving the slider seeks the TUI to that trace position.
- Capacity and fullness information in the force-tree and treemap views when
  that data is available in the trace.

## Troubleshooting

If the page is blank or cannot load the model:

- Confirm that `gwr-spotter` is still running with the binary trace loaded.
- Confirm that the `--no-server` flag has not been passed to `gwr-spotter`.
- Confirm that ports 8000 and 9991 are not being used by another process.
- Check the browser console for failed requests to `http://localhost:8000`.

<!-- ANCHOR_END: frontend_usage -->

[D3.js]: https://d3js.org
[frontend documentation]: #frontend-usage
[gwr-track]: ../gwr-track/README.md
