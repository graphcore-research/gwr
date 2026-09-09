<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# GWR Visualisation

`gwr-visualisation` generates a static web report for exploring gwr-timetable
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
- `data.json`: the exported visualisation data model
- `data.js`: a compact copy of the data embedded for direct file opening
- `core.js`, `palettes.js`, `filters.js`, `timetable-graph-layout.js`,
  `timetable-graph-viewport.js`, `timetable-graph.js`, `pe-grid.js`,
  `timetable.js`, `tensors.js`, `tensor-accesses.js`, `memory.js`,
  `relationships.js`, `workspace-model.js`, `workspace-settings.js`, and
  `workspace.js`: focused UI modules loaded in dependency order
- `app.js`: render orchestration, event wiring, and startup
- `style.css`: local report styling

## What The Report Shows

The report has five static-analysis views. The Layers view relates graph layers
to their compute and tensor traffic, the Compute view derives machine-op counts
from each compute node's operator and tensor views, the Memory view combines
timetable tensor traffic with the optional platform memory map, the Tensor view
focuses on tensor layout, detail, and PE traffic, and the Graph view supports
node-by-node dataflow navigation. The global Layer, Processing element, Tensor,
and Memory filters support multi-selection, with All and None shortcuts. Each
list can be narrowed with a case-insensitive regular expression and the matches
selected as a group. The filters apply to every visible panel, including the
headline totals, graph, tensor traffic, PE overview, and memory allocation
summaries. Selecting a layer in the Layers overview also updates the Layer
detail selection without changing the active filters.

The Graph view includes a deterministic Tensor and Compute dataflow graph and a
Selected node panel. Layers start as expanded compound nodes containing their
compute allocation. Compute IDs ending in `_part_<number>` start folded into
logical groups when their operation and graph layer also match. The graph uses a
compound layered layout. Layers occupy stable left-to-right columns. Tensors
produced and consumed entirely within one layer are placed between their local
producer and consumer inside that layer. Read-only inputs used by one layer are
alternated along its top and bottom edges to separate them from inter-layer
traffic. Graph outputs and cross-layer tensors remain as narrow vertical
rectangles in inter-layer gutters. Tensors read beyond the adjacent layer move
into alternating top and bottom bypass lanes, keeping their long connections
outside layers that do not consume them. Compute groups stack vertically inside
layers. Alternating barycentric sweeps reduce edge crossings, while explicit
side ports and stable curved channels keep links traceable. Layers and groups
can be selected for aggregate details, double-clicked, or explicitly expanded
and collapsed from Selected node. The graph supports pan, zoom, fit-to-panel,
layout reset, Expand visible, Expand all, and Collapse all. Expand visible opens
folded containers intersecting the current viewport; Expand all opens one
hierarchy level per click, expanding layers before compute groups, until every
container is open or the visible-node limit is reached. Collapse all similarly
folds compute groups first and layers on the following click. The graph uses
blue links for reads, yellow links for writes, and neutral dashed links for
compute control dependencies. Selection edge mode leaves unrelated links as
faint context while emphasizing links touching the selected or hovered node; All
edge mode raises the contrast of the complete topology. Tensor rectangles have a
minimum clickable height, then scale their height with byte size. Compute-node
area scales with static machine ops. Graph settings offers separate Tensor size
and Compute size multipliers from 0.1 to 10, with 1 retaining the original
sizes. These scale node dimensions, not the viewport zoom; compute sizing also
applies to folded groups. Layout spacing updates without resetting expansion or
selection, and each workspace remembers its multipliers. Folded compute-group
area can be scaled by either the number of visible compute nodes or their total
static machine ops; the selected mode is retained in the workspace. Compute
lanes can be disabled, grouped by operation, or grouped by each compute node's
dominant static machine operation. Lane and edge modes are retained in the
workspace. Expanded layers use inset headers for their names and aggregate
compute figures, while neutral lane bands preserve operation colour as the node
encoding. Expanded layer and group containers grow to contain their visible
members. Expanded compute groups use an aligned grid, adding columns when they
exceed the configured maximum rows. The maximum defaults to 32 and is retained
in the workspace. Layout is recalculated only when filters, expansion state, or
lane mode changes. A global topological ranking pass enforces left-to-right
placement for every visible DAG edge, including dependencies wholly within one
layer. Off-screen nodes and links are culled and hit testing uses a spatial
index.

The compact navigator inside the Timetable graph reuses its layout and fits the
complete visible graph into a compact navigation panel. A neutral overlay shows
the area currently visible in the main graph. Clicking or dragging in the
overview moves the main graph to that position without changing its zoom level.
Holding Control while dragging draws a range and fits that range into the main
graph on release. The overview stays fitted to the complete graph. Its mouse
wheel and + and - keys when focused zoom the main graph around its current
centre. The graph's toolbar provides zoom and Fit buttons. The visible-range
overlay updates as the main graph zooms.

Selecting a tensor anywhere in the report also selects it in Graph. Selecting a
compute access or graph compute node updates Graph, its PE, and its layer
without changing filters. Selecting a tensor, compute node, or folded compute
group also centres it in the graph while preserving the current zoom level.
Selected node lists immediate inputs and outputs, including ports and TensorView
geometry, and each neighbour can be selected to navigate through the timetable.
Existing Layer, PE, Tensor, and Memory filters hide excluded graph nodes.
Partition groups are expanded individually, with a 50,000-visible-node limit to
keep very large reports responsive.

When the graph canvas has focus, keyboard navigation starts on the selected
node's outputs. Up and Down move the highlighted neighbour, and Right follows
the highlighted output. Left switches to inputs; Up and Down then choose an
input, and a second Left follows it. The active direction is retained after
moving to another node. Enter expands or collapses the selected layer or compute
group. The Selected node panel outlines the active side and neighbour, while the
graph draws light dashed outlines around all visible inputs and outputs and a
darker outline around the active neighbour.

The Layers view includes:

- overall layer, tensor, compute-node, edge, and active-PE totals
- a selectable, sortable Layers overview with configurable columns for compute
  nodes, machine ops, reads, writes, tensors, and active PEs, plus optional
  columns for each compute-node type and machine-op type derived from the
  report; these retain the shared colours and show zero for absent types. It
  uses the same filter-style column picker and draggable dividers as the other
  overviews
- selected-layer details with machine-op and operator breakdowns
- a hierarchical edge-bundling panel for comparing layer-to-PE compute
  allocation, layer-to-memory tensor traffic, or PE-to-memory tensor traffic

The Compute view includes:

- total static machine ops, split by the machine-op types exported in the report
  data
- per-PE allocation totals, average and maximum allocation, and max/average
  imbalance
- a PE overview that switches between a single-measure platform grid and a
  configurable multi-column chart
- shared measures for compute allocation, total/read/written data, selected
  tensor traffic, or optional metrics
- a selected-PE comparison against the platform average and maximum

The Memory view includes:

- the PE overview, initially showing total data traffic as a sorted chart
- an overall Memory summary for capacity, allocation, reads, and writes
- a selectable, sortable Memories overview with configurable columns for
  capacity, allocation, traffic, tensor count, and memory kind; it uses the same
  filter-style column picker and draggable dividers as PE and Tensor overviews
- tensor memory regions by address, with an option to collapse large unused
  address gaps between regions
- a selectable per-memory layout
- selected-tensor size versus written/read byte totals

The Tensor view includes a Tensor overview, Tensor accesses, Tensor memory map,
Tensor detail, and PE overview. Tensor overview is a sortable, paginated
comparison table for tensor size, Read and Written bytes, Read and Written
percentages of tensor size, data type, and the number of PEs reading and writing
each tensor. Numeric columns share scales across the filtered tensors and mark
the filtered average, making unusually large, multiply-read,
incompletely-written, or widely-shared tensors visible. Columns can be added or
removed, and their header dividers can be dragged to change their relative
widths while the table continues to fill the panel. Double-clicking a divider
distributes the selected data columns evenly. The None action clears the data
columns so one replacement can be selected quickly. Those choices are saved with
the workspace.

Tensor accesses groups the selected tensor's compute-node accesses by PE. Read
accesses appear above Written accesses and share a tensor-relative coverage
scale. Selecting an access also selects its PE and shows how its TensorView
offset and extent map onto every logical tensor dimension and its physical
access pattern. The Physical runs control shows the first 4, 8, 16, 32, or 64
contiguous row-major byte runs. Double-clicking filters the report to that PE.
Complete PE groups are paged with a soft limit of 200 access rows. The generated
data keeps a normalized `compute_nodes` table; tensor `accesses` refer to that
table and to deduplicated tensor `views`. A missing view reference means the
full tensor.

Clicking a Tensor overview row selects its tensor and double-clicking filters
the report to it. Tensor detail presents size, traffic, percentage, data-type,
and reading/writing PE counts for the selected tensor, together with its address
and shape. In the Tensors workspace, the PE grid initially shows Read traffic
across filtered tensors. Double-click a tensor to restrict that traffic to it.

The interface provides six remembered task workspaces. New reports open Summary;
reopening a report restores its last workspace. Filters support multi-selection
and regular expressions and remain shared across workspace switches.

| Workspace | Primary                                  | Companion tabs                                  |
| --------- | ---------------------------------------- | ----------------------------------------------- |
| Summary   | Timetable, Compute, and Memory summaries | None                                            |
| Layers    | Layers overview                          | Relationships, PE overview                      |
| Dataflow  | Timetable graph                          | Relationships, Layers overview                  |
| Compute   | PE overview                              | Relationships, Layers overview                  |
| Memory    | Memories overview                        | Tensor memory map, PE overview, Relationships   |
| Tensors   | Tensor overview                          | Tensor accesses, Tensor memory map, PE overview |

Each region has tabs and a Views menu for adding, closing, ordering, or moving a
view to the other region. A view appears once per workspace. The primary region
always retains a tab. All summaries combines the three existing summary views;
opening an individual summary replaces the combined view to avoid duplication.

Automatic arrangement prefers the companion below the primary. It uses
side-by-side regions only when the available workspace is at least 1.5 times as
wide as it is tall and both regions fit their minimum widths. Narrow or short
workspaces fall back to tabs when the split cannot fit. Companion beside and
Companion below request an arrangement; when minimum usable dimensions cannot
fit, tabs take over without losing the saved proportions. Both arrangements
default to 50% primary and 50% companion, subject to minimum dimensions. Drag
dividers or use arrow keys when focused to resize. Double-click a divider to
restore its default. Maximize temporarily gives one view the available space;
Restore returns to the saved layout. Reset workspace resets only the current
workspace. The graph retains its zoom and expansion state during layout changes.

The Inspector replaces the separate detail panels and follows the last selected
entity. Tensor inspection includes tensor statistics and graph inputs/outputs;
compute/group inspection includes connections and expansion controls. Layer, PE,
and memory selections show their corresponding details. Selecting a compute may
highlight its PE and layer without replacing the compute inspector. Hover does
not change selection; double-click filtering retains its existing behavior. The
Inspector can be hidden and becomes a tab in narrow windows.

Workspace configurations are stored locally per timetable source, including
region tabs, active tabs, layout proportions, inspector visibility, chart
columns and sorting, relationship measures, and graph display settings. Legacy
panel settings migrate where compatible; old geometry is discarded and the old
storage entry is retained. Reports remain usable when storage is unavailable.
Selection, filters, temporary focus, graph positions and expansion remain
session-local. Inactive views retain dirty state and render when activated.

`workspace-model.js` owns default configurations, tab operations, responsive
arrangement decisions and storage migration. `workspace-settings.js` captures
and restores renderer settings. `workspace.js` owns the view registry, region
mounting, inspector composition, dividers, and persistence. `workspace.css`
contains the window-filling shell. Renderers keep their existing DOM roots and
are never cloned when switching workspaces.

Compute-node and machine-op totals use the same composition-strip component in
summary and detail panels. Stable colours identify each category, exact values
remain visible, and zero-value categories remain in the legend without taking
visual space. Read is always shown above Written on paired, shared-scale traffic
bars. In entity details, strips are scaled to the filtered peer maximum and a
thin marker shows the filtered average. Clicking a Machine ops, Compute nodes,
Read, Written, or individual machine-op label selects that measure in PE
overview.

Double-clicking a layer in Layers overview, a tensor in the Tensor memory map or
Memory detail, a PE in any PE overview layout, or a memory in Memories overview
filters the whole report to that entity. A single click only changes the current
selection.

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

The PE overview grid shows one selected measure. It can show compute allocation,
filtered data traffic for all tensors or the selected tensor, or metrics from an
overlay file. Grid values can be encoded using colour intensity or centred
square area while retaining the platform layout. Scaling can follow the filtered
PEs, the full timetable (the default), or an explicit fixed maximum, making
comparisons across filters repeatable.

PE traffic columns are explicitly labelled **All tensors: Read/Written** or
**Selected tensor: Read/Written**. Both respect active filters; selecting a
tensor changes only the selected-tensor measures. The chart names the selected
tensor whenever these columns are shown, and both scopes can be compared side by
side.

The overview lists support keyboard navigation: focus an overview row or its
active view tab and use Up/Down to walk through the current sorted, filtered
list, or Page Up/Down to move the selection by one visible page. This works in
Layers, PE, Memories, and Tensor overviews, updates the shared selection without
changing filters, and crosses tensor pages and layer windows. Inputs, column
controls, and graph navigation keep their own keyboard behaviour.

The PE overview chart is a configurable comparison table. Data columns can be
searched by case-insensitive regular expression in every overview's Columns
picker. Typing narrows the available choices without changing visible columns;
Select matches replaces the selection, and Clear removes only the search.
Invalid patterns leave the column selection unchanged. Columns can also be
selected with the same checkbox pattern as the report filters, removed down to
one, and resized by dragging their header dividers. The visible columns continue
to fill the panel. Double-clicking a divider distributes the selected data
columns evenly. The None action clears the data columns so one replacement can
be selected quickly. Timetable data and Metrics overlay shortcuts replace the
visible columns with those related sets; the metrics shortcut is disabled when
the report has no overlay values. Each numeric column has its own filtered scale
and average marker. Clicking a PE ID or metric heading sorts by that column, and
clicking it again reverses the order. Column and sort choices are saved with the
workspace. A shared state legend distinguishes selection, zero, unavailable,
missing, and filtered PEs. The grid Measure, encoding, and scale controls remain
visible while its content scrolls.

Entity hover is shared across panels, so hovering a layer, PE, tensor, or memory
highlights the same entity wherever it is visible. Relationship diagrams render
the strongest configurable number of links, report displayed and total counts,
and isolate incoming or outgoing links on hover. Clicking pins an entity through
the normal selection state; double-clicking filters to it.

Memory layouts always outline the allocated tensor range and can independently
show allocation, read coverage, write coverage, or the combined split view.
Collapsed address gaps use a visible break treatment, and tensors belonging to
the selected platform memory are highlighted in the timetable-wide map.

For larger reports, long rows use rendering containment, Layers overview renders
a navigable 200-layer window, and Tensor overview renders configurable pages of
100, 200, or 500 tensors. Panels report displayed versus available counts where
rendering is intentionally bounded.

Machine-op colours are supplied by the generated report and used consistently
across summaries, details, PE overview layouts, and relationships. Compute-node
operators use a stable categorical palette. Read remains blue and Written
remains yellow.

## Development

The browser code uses classic scripts and a shared
`window.GWR_VISUALISATION_APP` namespace so generated reports continue to work
when opened directly from disk. `core.js` owns shared state and utilities;
`palettes.js` owns stable categorical and data-type colour assignments;
`filters.js` owns filter state and aggregation; `timetable-graph-layout.js` owns
deterministic column ordering, geometry, routing, and spatial indexing;
`timetable-graph-viewport.js` owns dependency-free canvas pan, zoom, animation,
and coordinate transforms; `timetable-graph.js` owns graph construction, canvas
rendering, and selected-node navigation; `pe-grid.js` owns PE-overview measure
and layout selection; the timetable, tensor, memory, and relationship files own
their respective renderers; `workspace.js` owns region layout, tabs, visibility,
sizing, focus, and persistence; and `app.js` connects the modules without adding
a bundler or runtime dependency. Each module exposes only the functions required
by modules loaded later in that dependency order.

Useful checks for this crate:

```bash
cargo +nightly fmt
cargo check -p gwr-visualisation
cargo test -p gwr-visualisation
npx prettier --check gwr-visualisation/assets/*.js
npx eslint gwr-visualisation/assets gwr-visualisation/tests-js
node --test gwr-visualisation/tests-js/*.test.js
```

For browser regression checks, use an installed Playwright package and Chrome:

```bash
GWR_PLAYWRIGHT_MODULE=/path/to/playwright node gwr-visualisation/tests-js/workspace-browser.js /path/to/generated/report
```

Pass additional report directories to check small and large reports together.
The checks use direct file loading, without a server, and exercise workspace
switching, responsive layout, selection ownership, hidden rendering, and reload.

The repository's `prek` configuration runs Prettier and ESLint for changed
visualisation JavaScript before commits and merge commits. The development
dependency installer pins both tools so local and CI checks use the same
versions.

The Rust implementation follows the same focused-module structure. `lib.rs` owns
input loading and static bundle generation. `analysis/mod.rs` indexes the
timetable and orchestrates report construction, while its `compute`, `graph`,
`memory`, and `tensors` modules contain the corresponding domain logic.
`model.rs` contains the serialized report model, and `tests.rs` keeps the
analysis tests alongside those private implementation modules.
