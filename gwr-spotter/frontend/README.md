<!-- Copyright (c) 2025 Graphcore Ltd. All rights reserved. -->

# Visualisation Frontend

This frontend is a prototype for visualisation and interaction between a [D3].js
frontend and gwr-spotter having loaded a binary trace file.

[D3]: https://d3js.org

## Usage

### Create a trace

The first step is to create a binary trace by running a GWR-based simulation.
For example:

```bash
cargo run --release --bin sim-ring -- --binary --binary-file trace.bin
```

### Load binary in gwr-spotter

`gwr-spotter` is a utility for reading trace files but will also open a port for
this frontend to interact with:

```bash
cargo run --release --bin gwr-spotter -- --bin trace.bin
```

### Start the frontend

This frontend can be started using Python:

```bash
python3 -m http.server 9991 -d gwr-spotter/frontend
```

### Start the frontend

Open http://localhost:9991 in a web browser. This has only been tested with
Chrome and Safari.

You should see a graphical representation of the design along with a menu that
allows you to select a number of different visual representations.

You can select nodes and `gwr-spotter` will be updated to filter to that node.
