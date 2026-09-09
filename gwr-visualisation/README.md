<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# GWR Visualisation

`gwr-visualisation` generates a static web report for exploring `gwr-timetable`
YAML files. It parses the timetable, optional platform, and optional metric
overlay files, then writes a browser-based visualisation bundle to disk.

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
- `data.json`: the human-readable report data
- `payload.js`: separately gzip-compressed core report and tensor-detail JSON,
  plus the WASM module, encoded for direct `file://` loading
- `gwr_visualisation.js`: generated `wasm-bindgen` loading glue
- `bootstrap.js`: a small decoder and startup-error boundary
- `style.css`: local report styling

The production bundle contains no handwritten application JavaScript. Filtering,
aggregation, formatting, rendering, relationships, interaction handling, and
workspace persistence run in Rust/WASM. `payload.js` deliberately uses a classic
script instead of `fetch`, so an output directory remains portable and its
`index.html` can be opened directly without a web server. The bootstrap renders
the initial Summary from the smaller core payload, yields for layout, then
attaches tensor details before declaring the full application ready.

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

Timetables with cyclic data dependencies are rejected. Disconnected roots at the
beginning of the graph are treated as parallel. A disconnected layer root listed
after the graph has advanced beyond layer 1 is assumed to continue the model
sequence at the next layer, preserving source-order layer numbering for
timetable aliases.

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
    "ticks": {
      "label": "Ticks",
      "unit": "ticks"
    },
    "utilisation": {
      "label": "Utilisation",
      "unit": "%"
    }
  },
  "metrics_by_pe": {
    "pe_0_0": {
      "ticks": 12340,
      "utilisation": 78.5
    },
    "pe_0_1": {
      "ticks": 10890,
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
reported as warnings in `data.json` and in the web report. A metric that appears
in `metrics_by_pe` without a `metrics` entry is added to the Measure control
using its key as its label.

The PE overview has one Measure control shared by its chart and grid layouts. It
can show compute allocation, filtered data traffic, traffic for the selected
tensor, or metrics from an overlay file. Both layouts retain the active measure
when toggled and display the filtered average and maximum. The layout and
Measure controls remain visible while the chart or grid content scrolls.

## Development

The crate has two targets. The default `generator` feature builds the native
library and CLI. The `web` feature builds the browser runtime for
`wasm32-unknown-unknown`; it is separate from native timetable dependencies.
`src/model.rs` is the typed report contract shared by both.

The native generator parses a `TimetableFile` into a `TimetableGraph`. Graph
construction resolves ports and tensor views and validates the timetable before
either simulation or report analysis uses it. The report builder borrows that
graph and produces `ReportData`; it does not rebuild timetable connections or
tensor views. `src/address.rs` contains the half-open address-range operations
shared by native analysis and the browser.

The browser code is divided by responsibility:

- `web/state.rs` owns filters and serializable interaction state.
- `web/address.rs` owns tensor-transfer arithmetic and memory geometry.
- `web/logic.rs` owns indexed lookup, cached aggregation, and filtered report
  totals.
- `web/relationships.rs` filters and bounds the relationship model before it is
  rendered.
- `web/render.rs` and its `render/` children own panel markup and drawing.
- `web/workspace.rs` owns version-1 local-storage restoration and panel layout.
- `web/app.rs` owns event delegation and coordinates state with visible panels.
- `payload.rs` owns deterministic gzip encoding and decoding.

High-cardinality filter lists are constructed only when opened and display a
500-item window when more than 1,000 values match. Layer comparisons display a
deterministic 500-layer window, and relationship plots retain at most 500
matching sources and the 5,000 strongest links. The selected source is retained,
and omitted source and link counts are shown separately. Filtered contexts and
summaries are cached by state generation. Hidden panels are not rendered, and
selection or mode changes only rerender panels that depend on the changed state.

### Generated WASM assets

Install the pinned target and generator, then rebuild the committed runtime:

```bash
rustup target add wasm32-unknown-unknown
cargo install --locked wasm-bindgen-cli --version 0.2.126
./gwr-visualisation/scripts/build-wasm.sh
```

The Rust dependency and CLI are both pinned to `wasm-bindgen` 0.2.126. CI checks
that committed files in `assets/generated/` match a clean release build:

```bash
./gwr-visualisation/scripts/build-wasm.sh --check
```

Current Safari, Chromium, and browsers with equivalent WebAssembly, gzip stream,
DOM, canvas, and local-storage support can open reports. Startup failures are
rendered into the document instead of leaving a blank page.

### Browser tests and performance

Install the pinned browser harness dependencies with Node.js 22 or newer:

```bash
npm ci --prefix gwr-visualisation/benchmarks
```

Exercise a report directly from a `file://` URL in Chromium and installed macOS
Safari:

```bash
npm --prefix gwr-visualisation/benchmarks run browser-test -- \
  --timetable /absolute/path/to/timetable.yaml \
  --platform /absolute/path/to/platform.yaml \
  --browsers chromium,safari \
  --output /tmp/gwr-visualisation-browser-test
```

The deterministic checks cover presets, filters, single- and double-click
selection, PE chart and grid modes, relationships, tensor and memory views,
panel controls, workspace restoration, and startup failures. Screenshots and a
JSON record are retained as diagnostic artifacts without a pixel-difference
gate.

Run the performance harness with two warm-ups and ten measurements per browser:

```bash
npm --prefix gwr-visualisation/benchmarks run benchmark -- \
  --timetable /absolute/path/to/partitioned-izi-gpt-oss20b.yaml \
  --browsers chromium,safari \
  --warmups 2 \
  --runs 10 \
  --session-attempts 3 \
  --output /tmp/gwr-visualisation-benchmark
```

The harness creates a fresh automation session and unique report path for every
sample, and runs Safari serially. It records raw JSON, a Markdown median table,
browser and OS versions, hardware, and the configuration. Cold startup ends when
the initial Summary has rendered. Interactions use a deterministic 64-layer
slice by default; override it with `--interaction-layer-pattern REGEX`.

Without a baseline, the harness records timings and validates deterministic
kernel checksums but does not apply a performance gate. To reject regressions,
pass a previous `benchmark.json`; the default limit is 10% for every metric:

```bash
npm --prefix gwr-visualisation/benchmarks run benchmark -- \
  --timetable /absolute/path/to/partitioned-izi-gpt-oss20b.yaml \
  --browsers chromium,safari \
  --baseline /absolute/path/to/benchmark.json \
  --max-regression-percent 10 \
  --output /tmp/gwr-visualisation-benchmark
```

The baseline must contain every requested browser and metric, and must use the
same timetable, platform, overlay, kernel iteration count, and interaction
pattern. Input files are compared by SHA-256 digest rather than pathname.
Browser, operating system, and hardware differences are reported as warnings
because they can make timing comparisons unreliable.

Chromium uses Playwright with installed Google Chrome and opens the generated
report directly from its `file://` URL. Safari uses Selenium with the real
`/usr/bin/safaridriver`, not Playwright WebKit. Safari's isolated automation
windows reject local-file navigation, so the harness serves the same generated
report over the loopback interface for Safari sessions. Safari requires macOS
and Develop > Allow Remote Automation. Opening a generated report directly in
Safari also requires Develop > Disable Local File Restrictions. The harness does
not clear or modify the user's normal Safari profile.

Useful checks for this crate:

```bash
cargo +nightly fmt
cargo check -p gwr-visualisation
cargo test -p gwr-visualisation
cargo clippy-strict
./gwr-visualisation/scripts/build-wasm.sh --check
prek run --all-files
```

`generator.rs` owns input loading and static bundle generation.
`analysis/mod.rs` builds report data from the validated timetable graph, while
its child modules calculate compute, tensor, memory, platform, and graph
summaries. Run nightly formatting because the repository CI and contributor
guidance use the nightly formatter.
