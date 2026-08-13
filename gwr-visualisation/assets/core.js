// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = (window.GWR_VISUALISATION_APP = {});

  const data = window.GWR_VISUALISATION_DATA;
  const {
    addressRange,
    intersectRanges,
    rangeUnionBytes,
    trafficForAccesses,
    retainedFocus,
    contextTensorCount,
  } = window.GWR_VISUALISATION_VIEW_MODEL;
  const fmt = new Intl.NumberFormat("en");

  const controls = {
    layer: document.getElementById("layer-filter"),
    pe: document.getElementById("pe-filter"),
    memory: document.getElementById("memory-filter"),
    tensor: document.getElementById("tensor-filter"),
    layerSummary: document.getElementById("layer-filter-summary"),
    peSummary: document.getElementById("pe-filter-summary"),
    memorySummary: document.getElementById("memory-filter-summary"),
    tensorSummary: document.getElementById("tensor-filter-summary"),
  };
  const viewControls = {
    layout: document.getElementById("view-layout"),
    views: document.getElementById("views"),
    toggles: [...document.querySelectorAll("[data-view-toggle]")],
    presets: [...document.querySelectorAll("[data-preset]")],
    panels: [...document.querySelectorAll("[data-view]")],
    addView: document.getElementById("workspace-add-view"),
    addButton: document.getElementById("workspace-add"),
    resetButton: document.getElementById("workspace-reset"),
  };
  const grid = document.getElementById("pe-grid");
  const rowAxis = document.getElementById("row-axis");
  const colAxis = document.getElementById("col-axis");
  const selectedPanel = document.getElementById("selected-pe");
  const timetableSummary = document.getElementById("timetable-summary");
  const layerSummary = document.getElementById("layer-summary");
  const layerDetail = document.getElementById("layer-detail");
  const relationshipBundle = document.getElementById("relationship-bundle");
  const relationshipControls = {
    mode: document.getElementById("relationship-mode"),
    measure: document.getElementById("relationship-measure"),
    strength: document.getElementById("relationship-strength"),
    strengthValue: document.getElementById("relationship-strength-value"),
  };
  const computeSummary = document.getElementById("compute-summary");
  const tensorMemory = document.getElementById("tensor-memory");
  const skipMemoryGaps = document.getElementById("skip-memory-gaps");
  const memorySummary = document.getElementById("memory-summary");
  const memoriesOverview = document.getElementById("memories-overview");
  const memoryDetail = document.getElementById("memory-detail");
  const selectedTensorPanel = document.getElementById("selected-tensor");
  const state = {
    selectedPe: data.pes.find((pe) => pe.total_nodes > 0) || data.pes[0],
    selectedTensor: data.tensors?.[0] || null,
    selectedLayerName: data.layers?.[0]?.name || null,
    selectedMemoryName: data.memory?.platform_memories?.[0]?.name || null,
    renderedTensorMemoryKey: null,
    renderedMemorySummaryKey: null,
    renderedMemoriesOverviewKey: null,
    renderedMemoryDetailKey: null,
  };
  const pesByName = new Map(data.pes.map((pe) => [pe.name, pe]));
  const tensorsById = new Map(
    (data.tensors || []).map((tensor) => [tensor.id, tensor]),
  );
  const memoryLayoutCache = new Map();
  const memoryMetricsCache = new Map();
  const filterContextCache = new Map();
  const relationshipModelCache = new Map();
  const viewPresets = {
    summary: ["timetable-summary", "compute-summary", "memory-summary"],
    layers: [
      "timetable-summary",
      "layer-summary",
      "layer-details",
      "relationships",
    ],
    compute: ["compute-summary", "pe-grid", "relationships", "selected-pe"],
    memory: [
      "memory-summary",
      "memories-overview",
      "relationships",
      "pe-grid",
      "memory-details",
      "tensor-memory",
      "selected-tensor",
    ],
    tensor: ["tensor-memory", "selected-tensor", "pe-grid"],
  };

  document.getElementById("source-path").textContent = data.summary.timetable;

  function option(value, label) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = label;
    return option;
  }

  function labelFromName(name) {
    const words = String(name).replaceAll("_", " ");
    return words.charAt(0).toUpperCase() + words.slice(1);
  }

  const machineOpTypes = (
    data.machine_ops?.length
      ? data.machine_ops
      : [
          ...new Set(
            (data.layers || []).flatMap((layer) =>
              Object.keys(layer.machine_ops || {}),
            ),
          ),
        ]
          .filter((name) => name !== "total")
          .map((name) => ({ name, label: labelFromName(name) }))
  )
    .map((op) => ({ name: op.name, label: op.label || labelFromName(op.name) }))
    .sort((left, right) => left.label.localeCompare(right.label));
  const machineOpKeys = machineOpTypes.map((op) => op.name);

  function emptyMachineOps() {
    return Object.fromEntries([
      ["total", 0n],
      ...machineOpKeys.map((name) => [name, 0n]),
    ]);
  }

  function formatCount(value) {
    if (typeof value === "number" && !Number.isInteger(value)) {
      return fmt.format(value);
    }
    const integer = toBigInt(value);
    if (integer < 10000n) {
      return fmt.format(integer);
    }
    return new Intl.NumberFormat("en", {
      notation: "compact",
      maximumFractionDigits: 2,
    }).format(integer);
  }

  function escapeHtml(value) {
    return String(value)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#39;");
  }

  function metricBreakdownMarkup(label, total, entries, showZero = false) {
    const items = entries.filter(
      ([, value]) => showZero || toBigInt(value) > 0n,
    );
    const ariaBreakdown = items
      .map(
        ([name, value, formatter = formatCount]) =>
          `${formatter(value)} ${name}`,
      )
      .join(", ");
    return `
    <div class="metric-breakdown-summary" aria-label="${escapeHtml(`${fmt.format(total)} ${label}${ariaBreakdown ? `: ${ariaBreakdown}` : ""}`)}">
      <div class="total"><span>${escapeHtml(label)}</span><strong>${formatCount(total)}</strong></div>
      ${items.map(([name, value, formatter = formatCount]) => `<div><span>${escapeHtml(name)}</span><strong>${formatter(value)}</strong></div>`).join("")}
    </div>
  `;
  }

  function machineOpsMarkup(ops = {}) {
    const entries = machineOpTypes.map((op) => [
      op.label,
      toBigInt(ops[op.name]),
    ]);
    const total = toBigInt(
      ops.total ?? entries.reduce((sum, [, value]) => sum + value, 0n),
    );
    return metricBreakdownMarkup("Machine ops", total, entries, true);
  }

  function computeNodesMarkup(total, byOp = {}) {
    const entries = data.ops
      .map((op) => [op, Number(byOp[op] || 0)])
      .sort((left, right) => left[0].localeCompare(right[0]));
    return metricBreakdownMarkup(
      "Compute nodes",
      Number(total || 0),
      entries,
      true,
    );
  }

  function comparisonMaxima(metrics) {
    return {
      nodes: Math.max(...metrics.map((metric) => Number(metric.nodes || 0)), 1),
      ops: metrics.reduce(
        (maximum, metric) => bigIntMax(maximum, toBigInt(metric.ops)),
        1n,
      ),
      read: metrics.reduce(
        (maximum, metric) => bigIntMax(maximum, toBigInt(metric.read)),
        1n,
      ),
      write: metrics.reduce(
        (maximum, metric) => bigIntMax(maximum, toBigInt(metric.write)),
        1n,
      ),
    };
  }

  function comparisonBarsMarkup(metric, maxima) {
    return comparisonMetricsMarkup([
      {
        label: "Compute nodes",
        value: metric.nodes,
        formatted: fmt.format(metric.nodes),
        mode: "nodes",
        maximum: maxima.nodes,
      },
      {
        label: "Machine ops",
        value: metric.ops,
        formatted: formatCount(metric.ops),
        mode: "ops",
        maximum: maxima.ops,
      },
      {
        label: "Read",
        value: metric.read,
        formatted: formatBytes(metric.read),
        mode: "read",
        maximum: maxima.read,
      },
      {
        label: "Written",
        value: metric.write,
        formatted: formatBytes(metric.write),
        mode: "write",
        maximum: maxima.write,
      },
    ]);
  }

  function comparisonMetricsMarkup(items, extraClass = "") {
    return `
    <div class="comparison-metrics ${extraClass}">
      ${items
        .map(
          ({ label, value, formatted, mode, maximum, marker }) => `
        <div class="comparison-metric">
          <div><span>${escapeHtml(label)}</span><strong>${formatted}</strong></div>
          <div class="comparison-track ${mode}">
            <div style="width: ${ratioPercent(value, maximum)}%"></div>
            ${marker === undefined ? "" : `<i style="left: ${Math.min(Math.max(Number(marker || 0), 0), 100)}%" aria-hidden="true"></i>`}
          </div>
        </div>
      `,
        )
        .join("")}
    </div>
  `;
  }

  function dims() {
    if (data.platform) {
      const platformRows = Number(data.platform.rows || 0);
      const platformCols = Number(data.platform.cols || 0);
      const fabricRows = (data.platform.fabrics || []).map((fabric) =>
        Number(fabric.rows || 0),
      );
      const fabricCols = (data.platform.fabrics || []).map((fabric) =>
        Number(fabric.cols || 0),
      );
      const peRows = data.pes.map((pe) => Number(pe.row) + 1);
      const peCols = data.pes.map((pe) => Number(pe.col) + 1);
      return [
        Math.max(platformRows, ...fabricRows, ...peRows, 1),
        Math.max(platformCols, ...fabricCols, ...peCols, 1),
      ];
    }

    const peRows = data.pes.map((pe) => Number(pe.row) + 1);
    const peCols = data.pes.map((pe) => Number(pe.col) + 1);
    return [Math.max(...peRows, 1), Math.max(...peCols, 1)];
  }

  function toBigInt(value, fallback = 0n) {
    if (value === undefined || value === null || value === "") {
      return fallback;
    }
    return BigInt(value);
  }

  function bigIntMax(left, right) {
    return left > right ? left : right;
  }

  function bigIntMin(left, right) {
    return left < right ? left : right;
  }

  function bigIntCompare(left, right) {
    const leftValue = toBigInt(left);
    const rightValue = toBigInt(right);
    return leftValue < rightValue ? -1 : leftValue > rightValue ? 1 : 0;
  }

  function bigIntToNumber(value) {
    return Number(value);
  }

  function ratioPercent(value, maximum) {
    const denominator = Number(maximum);
    if (denominator === 0) {
      return 0;
    }
    return (Number(value) / denominator) * 100;
  }

  function integerAverage(total, count) {
    return count ? Number(total) / count : 0;
  }

  function clipTensorToMemory(tensor, memory) {
    const [tensorStart, tensorEnd] = addressRange(
      tensor.addr,
      bigIntMax(toBigInt(tensor.num_bytes), 1n),
    );
    const [memoryStart, memoryEnd] = addressRange(
      memory.base_addr,
      memory.capacity_bytes,
    );
    const start = bigIntMax(tensorStart, memoryStart);
    const end = bigIntMin(tensorEnd, memoryEnd);
    if (end <= start) {
      return null;
    }
    return {
      id: tensor.id,
      addr: start.toString(),
      num_bytes: (end - start).toString(),
      tensor,
    };
  }

  function formatHex(value) {
    return `0x${toBigInt(value).toString(16)}`;
  }

  function formatBytes(bytes) {
    const units = ["B", "KiB", "MiB", "GiB"];
    if (typeof bytes === "number" && !Number.isInteger(bytes)) {
      let value = bytes;
      let unit = units[0];
      for (let index = 0; index < units.length - 1 && value >= 1024; index++) {
        value /= 1024;
        unit = units[index + 1];
      }
      const precision = value >= 10 || unit === "B" ? 0 : 1;
      return `${value.toFixed(precision)} ${unit}`;
    }
    const value = toBigInt(bytes);
    let divisor = 1n;
    let unitIndex = 0;
    while (unitIndex < units.length - 1 && value >= divisor * 1024n) {
      divisor *= 1024n;
      unitIndex += 1;
    }
    if (unitIndex === 0) {
      return `${fmt.format(value)} ${units[unitIndex]}`;
    }
    const whole = value / divisor;
    if (whole >= 10n) {
      const rounded = (value + divisor / 2n) / divisor;
      return `${fmt.format(rounded)} ${units[unitIndex]}`;
    }
    const roundedTenths = (value * 10n + divisor / 2n) / divisor;
    return `${fmt.format(roundedTenths / 10n)}.${roundedTenths % 10n} ${units[unitIndex]}`;
  }

  Object.assign(App, {
    data,
    fmt,
    controls,
    viewControls,
    grid,
    rowAxis,
    colAxis,
    selectedPanel,
    timetableSummary,
    layerSummary,
    layerDetail,
    relationshipBundle,
    relationshipControls,
    computeSummary,
    tensorMemory,
    skipMemoryGaps,
    memorySummary,
    memoriesOverview,
    memoryDetail,
    selectedTensorPanel,
    state,
    pesByName,
    tensorsById,
    memoryLayoutCache,
    memoryMetricsCache,
    filterContextCache,
    relationshipModelCache,
    viewPresets,
    option,
    labelFromName,
    machineOpTypes,
    machineOpKeys,
    emptyMachineOps,
    formatCount,
    escapeHtml,
    metricBreakdownMarkup,
    machineOpsMarkup,
    computeNodesMarkup,
    comparisonMaxima,
    comparisonBarsMarkup,
    comparisonMetricsMarkup,
    dims,
    toBigInt,
    bigIntMax,
    bigIntCompare,
    bigIntToNumber,
    ratioPercent,
    integerAverage,
    addressRange,
    intersectRanges,
    rangeUnionBytes,
    trafficForAccesses,
    retainedFocus,
    contextTensorCount,
    clipTensorToMemory,
    formatHex,
    formatBytes,
  });
})();
