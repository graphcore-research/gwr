<!-- Copyright (c) 2025 Graphcore Ltd. All rights reserved. -->

# Visualisation Frontend

This frontend is a prototype for visualisation and interaction between a [D3].js
frontend and tramway-spotter having loaded a binary trace file.

[D3]: https://d3js.org

## Usage

### Create a trace

The first step is to create a binary trace by running a TRAMWAY-based
simulation. For example:

```bash
cargo run --release --bin sim-ring -- --binary --binary-file trace.bin
```

### Load binary in tramway-spotter

`tramway-spotter` is a utility for reading trace files but will also open a port
for this frontend to interact with:

```bash
cargo run --release --bin tramway-spotter -- --bin trace.bin
```

### Start the frontend

This frontend can be started using Python:

```bash
cd frontend/
python3 -m http.server 9991
```

### Start the frontend

Open http://localhost:9991 in a web browser. This has only been tested with
Chrome and Safari.

You should see a graphical representation of the design along with a menu that
allows you to select a number of different visual representations.

In the `force_tree` view you can select nodes and `tramway-spotter` will be
updated to select that node.
