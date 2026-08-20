<!-- Copyright (c) 2025 Graphcore Ltd. All rights reserved. -->

# Visualisation Frontend

This directory contains a static [D3.js] frontend for visualising and
interacting with a model loaded into `gwr-spotter` from a binary trace.

[D3.js]: https://d3js.org

<!-- ANCHOR: frontend_usage -->

## Usage

Run all commands below from the workspace root. The frontend and `gwr-spotter`
run as separate processes on the same machine.

### 1. Create a trace

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

### 3. Serve the frontend

Serve the static frontend files on a second loopback port using Python:

```bash
python3 -m http.server 9991 --bind 127.0.0.1 --directory gwr-spotter/frontend
```

### 4. Open the frontend

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
- Confirm that ports 8000 and 9991 are not being used by another process.
- Check the browser console for failed requests to `http://localhost:8000`.

<!-- ANCHOR_END: frontend_usage -->
