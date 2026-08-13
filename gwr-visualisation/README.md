<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# GWR Visualisation

`gwr-visualisation` generates a static web report for exploring gwr-timetable
YAML files. The timetable, optional platform, and optional metric overlay files
are parsen, then writes a browser-based visualisation bundle that can be opened
is written to disk.

## Usage

From the workspace root:

```bash
cargo run -p gwr-visualisation -- \
  --timetable path/to/timetable.yaml \
  --out path/to/output-dir
```

To include platform metadata and open the generated report:

```bash
cargo run -p gwr-visualisation -- \
  --timetable path/to/timetable.yaml \
  --platform path/to/platform.yaml \
  --out /tmp/gwr-vis \
  --open
```

The output directory will contain:

- `index.html`: the static report entry point
- `data.json`: the exported visualisation data model
- `data.js`: a compact copy of the data embedded for direct file opening
- `view-model.js`, `core.js`, `filters.js`, `pe-grid.js`, `timetable.js`,
  `tensors.js`, `memory.js`, `relationships.js`, and `workspace.js`: focused UI
  modules loaded in dependency order
- `app.js`: render orchestration, event wiring, and startup
- `style.css`: local report styling

## What The Report Shows

The report has four static-analysis views. The Layers view relates graph layers
to their compute and tensor traffic, the Compute view derives machine-op counts
from each compute node's operator and tensor views, the Memory view combines
timetable tensor traffic with the optional platform memory map, and the Tensor
view focuses on tensor layout, detail, and PE traffic. The global Layer and
Processing element filters support any combination of layers and PEs, with All
and None shortcuts. Each list can be narrowed with a case-insensitive regular
expression and the matches selected as a group. The filters apply to every
visible panel, including the headline totals, tensor traffic, PE overview, and
memory allocation summaries. Selecting a layer in the Layers summary also
updates the Layer detail selection without changing the active filters.

The Layers view includes:

- overall layer, read, and write totals, plus compute-node and machine-op
  breakdowns
- a selectable Layers summary with shared scales for compute nodes, machine ops,
  reads and writes
- selected-layer details with machine-op and operator breakdowns
- per-PE compute-node, machine-op, read, and write comparison rows for the
  selected layer
- a hierarchical edge-bundling panel for comparing layer-to-PE compute
  allocation, layer-to-memory tensor traffic, or PE-to-memory tensor traffic

The Compute view includes:

- total static machine ops, split by the machine-op types exported in the report
  data
- per-PE allocation totals, average and maximum allocation, and max/average
  imbalance
- a PE overview that switches between platform grid and sorted chart layouts
  while retaining the selected measure
- shared measures for compute allocation, total/read/written data, selected
  tensor traffic, or optional metrics
- a selected-PE comparison against the platform average and maximum

The Memory view includes:

- the PE overview, initially showing total data traffic as a sorted chart
- an overall Memory summary for capacity, allocation, reads, and writes
- a selectable Memories overview comparing allocation and traffic per memory
- exact view traffic attribution and allocation totals that count aliased
  address ranges once
- tensor memory regions by address, with an option to collapse large unused
  address gaps between regions
- a selectable per-memory layout
- selected-tensor size versus written/read byte totals

The Tensor view includes the Tensor memory map, Tensor detail, and PE overview.
The PE overview initially shows where the selected tensor is read on the PE
grid.

The Summary preset shows the timetable, compute, and memory summaries together.
Layer, PE, memory, and tensor filters support multi-selection and
regular-expression matching. The workspace supports responsive, one-, two-, and
three-column layouts. Panels can be added, hidden, reordered by dragging or
keyboard-friendly move controls, collapsed, focused, resized vertically, and
assigned one-column, two-column, or full-row widths. Workspace configuration is
restored from browser-local storage for each timetable source. New reports start
with the Summary views in a one-column layout. Static machine-op counts show an
approximation for how well-balanced the timetable file is across PEs. A metrics
overlay file can give runtime performance metrics from simulations.

Double-clicking a layer in Layers summary, a tensor in the Tensor memory map or
Memory detail, a PE in Layer detail or either PE overview layout, or a memory in
Memories overview filters the whole report to that entity. A single click only
changes the current selection.

The Relationships panel follows Danny Holten's hierarchical edge-bundling
technique: connections are drawn as splines pulled toward the path through their
layer and resource hierarchy. The bundle-strength control interpolates between
direct links and fully bundled hierarchy paths. See
[Hierarchical Edge Bundles: Visualization of Adjacency Relations in Hierarchical Data](https://doi.org/10.1109/TVCG.2006.147).
The relationship modes respect the global Layer and PE filters. They show
layer-to-PE compute, layer-to-memory and PE-to-memory traffic, individual tensor
allocations across platform memories, and the PEs that read or write each
tensor. Tensor leaves are ordered by their producing layer (or first consuming
layer for input tensors) so each layer forms a contiguous hierarchy branch.
Memory relationships can show bytes read or written; tensor-to-PE relationships
can switch between reads and writes. Click or keyboard-activate any relationship
leaf to select its layer, PE, memory, or tensor and update the corresponding
detail views. Double-clicking a relationship leaf instead sets the corresponding
global filter to that entity.

Layers are derived from timetable data dependencies. Tensors with no incoming
data edge are graph roots. Compute nodes that consume those root tensors advance
the layer depth, while other compute nodes inherit the current layer. If no
compute node consumes a root tensor, every compute node starts a layer.

The analysis assumes that data dependencies are acyclic. Disconnected roots at
the beginning of the graph are treated as parallel. A disconnected layer root
listed after the graph has advanced beyond layer 1 is assumed to continue the
model sequence at the next layer; this handles timetable aliases without a
general graph-repair heuristic.

If no platform is provided, the report is still generated from timetable PE
names such as `pe_3_17`.

For example, to inspect tensor placement and PE consumers for the ResNet
partitioned timetable:

```bash
cargo run -p gwr-visualisation -- \
  --timetable path/to/timetable.yaml \
  --platform path/to/platform.yaml \
  --out /tmp/gwr-resnet-vis \
  --open
```

## Overlay Metrics

Metric overlays are optional JSON files. They let external performance reports
attach numeric values to PE names without requiring this tool to parse every
report format directly.

Example:

```json
{
  "metrics": {
    "cycles": {
      "label": "Cycles",
      "unit": "cycles"
    },
    "utilisation": {
      "label": "Utilisation",
      "unit": "%"
    }
  },
  "metrics_by_pe": {
    "pe_0_0": {
      "cycles": 12340,
      "utilisation": 78.5
    },
    "pe_0_1": {
      "cycles": 10890,
      "utilisation": 71.2
    }
  }
}
```

Run with:

```bash
cargo run -p gwr-visualisation -- \
  --timetable path/to/timetable.yaml \
  --platform path/to/platform.yaml \
  --overlay path/to/overlay.json \
  --out /tmp/gwr-vis
```

Overlay PE names must match timetable or platform PE names. Unknown PE names are
reported as warnings in `data.json` and in the web report.

The PE overview has one Measure control shared by its chart and grid layouts. It
can show compute allocation, filtered data traffic, traffic for the selected
tensor, or metrics from an overlay file. Both layouts retain the active measure
when toggled and display the filtered average and maximum. The layout and
Measure controls remain visible while the chart or grid content scrolls.

## Development

The browser code uses classic scripts and a shared
`window.GWR_VISUALISATION_APP` namespace so generated reports continue to work
when opened directly from disk. `view-model.js` provides browser-independent
range, traffic, and focus helpers; `core.js` owns shared state and utilities;
`filters.js` owns filter state and aggregation; `pe-grid.js` owns PE-overview
measure and layout selection; the timetable, tensor, memory, and relationship
files own their respective renderers; `workspace.js` owns panel layout,
ordering, visibility, sizing, focus, and persistence; and `app.js` connects the
modules without adding a bundler or runtime dependency. Each module exposes only
the functions required by modules loaded later in that dependency order.

Useful checks for this crate:

```bash
cargo +nightly fmt
cargo check -p gwr-visualisation
cargo test -p gwr-visualisation
node --test gwr-visualisation/tests/view-model.test.mjs
npx prettier --check gwr-visualisation/assets/*.js
npx eslint gwr-visualisation/assets
```

The repository's `prek` configuration runs the browser-independent tests,
Prettier, and ESLint for changed visualisation JavaScript before commits and
merge commits. The development dependency installer pins both tools so local and
CI checks use the same versions.

The Rust implementation follows the same focused-module structure. `lib.rs` owns
input loading and static bundle generation. `analysis/mod.rs` indexes the
timetable and orchestrates report construction, while its `compute`, `graph`,
`memory`, and `tensors` modules contain the corresponding domain logic.
`model.rs` contains the serialized report model, and `tests.rs` keeps the
analysis tests alongside those private implementation modules.
