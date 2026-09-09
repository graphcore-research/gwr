// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    data,
    state,
    machineOpTypes,
    machineOpsFor,
    allFilter,
    isAllFilter,
    peFilterValue,
    filterMatches,
    filteredLayers,
    filteredTensors,
    tensorTraffic,
    tensorTrafficFor,
    scaleTensorToMemory,
    option,
    labelFromName,
    formatCount,
    formatBytes,
    escapeHtml,
    toBigInt,
    bigIntCompare,
    integerAverage,
    createOverviewColumnConfiguration,
  } = App;

  const peOverviewControls = {
    measure: document.getElementById("pe-overview-measure"),
    measureControl: document.getElementById("pe-overview-measure-control"),
    modes: [...document.querySelectorAll("[data-pe-overview-mode]")],
    gridEncodings: [...document.querySelectorAll("[data-pe-grid-encoding]")],
    gridEncodingGroup: document.getElementById("pe-grid-encoding"),
    chart: document.getElementById("pe-overview-chart"),
    grid: document.getElementById("pe-overview-grid"),
    legend: document.getElementById("pe-overview-legend"),
    scale: document.getElementById("pe-overview-scale"),
    scaleControl: document.getElementById("pe-overview-scale-control"),
    fixedScale: document.getElementById("pe-overview-fixed-scale"),
    fixedMaximum: document.getElementById("pe-overview-fixed-maximum"),
    stateLegend: document.getElementById("pe-visual-state-legend"),
    mode: "grid",
    gridEncoding: "colour",
    chartOrder: "pe-asc",
    chartSortKey: "pe",
    chartSortDirection: "asc",
    scaleMode: "global",
  };
  const overlayNumber = new Intl.NumberFormat("en", {
    maximumFractionDigits: 2,
  });
  function cssColour(colour) {
    return colour.startsWith("--") ? `var(${colour})` : colour;
  }

  function formatDurationNs(value) {
    const nanoseconds = Number(value || 0);
    if (Math.abs(nanoseconds) >= 1e9) {
      return `${overlayNumber.format(nanoseconds / 1e9)} s`;
    }
    if (Math.abs(nanoseconds) >= 1e6) {
      return `${overlayNumber.format(nanoseconds / 1e6)} ms`;
    }
    if (Math.abs(nanoseconds) >= 1e3) {
      return `${overlayNumber.format(nanoseconds / 1e3)} us`;
    }
    return `${overlayNumber.format(nanoseconds)} ns`;
  }

  function formatUnit(value, unit) {
    if (unit === "ns") {
      return formatDurationNs(value);
    }
    if (unit === "bytes") {
      return formatBytes(value);
    }
    const formatted = overlayNumber.format(value || 0);
    return unit === "%"
      ? `${formatted}%`
      : `${formatted}${unit ? ` ${escapeHtml(unit)}` : ""}`;
  }

  function computeNodeValues(pes, scope = "filtered") {
    const values = new Map(pes.map((pe) => [pe.name, 0]));
    const layers = scope === "global" ? data.layers : filteredLayers();
    for (const layer of layers) {
      for (const pe of layer.pes || []) {
        if (values.has(pe.name)) {
          values.set(
            pe.name,
            Number(values.get(pe.name)) + Number(pe.compute_nodes || 0),
          );
        }
      }
    }
    return values;
  }

  function trafficValues(tensors, direction, scope = "filtered") {
    const values = new Map();
    for (const tensor of tensors) {
      const traffic =
        scope === "global"
          ? tensorTrafficFor(tensor, allFilter, allFilter)
          : tensorTraffic(tensor);
      let connections = traffic.writes;
      if (direction === "read") {
        connections = traffic.reads;
      } else if (direction === "total") {
        connections = [...traffic.reads, ...traffic.writes];
      }
      for (const connection of connections) {
        values.set(
          connection.pe,
          toBigInt(values.get(connection.pe)) +
            scaleTensorToMemory(
              tensor,
              connection.bytes,
              scope === "global" ? allFilter : undefined,
            ),
        );
      }
    }
    return values;
  }

  function staticMeasures() {
    return [
      {
        value: "compute:machine-ops",
        group: "Compute allocation",
        label: "Machine ops",
        colour: "--activity",
        metricValue: (pe, scope) =>
          toBigInt(
            machineOpsFor(pe, scope === "global" ? allFilter : undefined)
              ?.total,
          ),
        integer: true,
        format: formatCount,
      },
      {
        value: "compute:compute-nodes",
        group: "Compute allocation",
        label: "Compute nodes",
        colour: "--activity",
        values: computeNodeValues,
        format: formatCount,
      },
      ...machineOpTypes.map((machineOp) => ({
        value: `compute:machine-op:${machineOp.name}`,
        group: "Compute allocation",
        label: machineOp.label,
        colour: machineOp.colour,
        metricValue: (pe, scope) =>
          toBigInt(
            machineOpsFor(pe, scope === "global" ? allFilter : undefined)?.[
              machineOp.name
            ],
          ),
        integer: true,
        format: formatCount,
      })),
      {
        value: "data:total",
        group: "Data",
        label: "All tensors: Total",
        colour: "--activity",
        values: (_, scope) =>
          trafficValues(
            scope === "global" ? data.tensors : filteredTensors(),
            "total",
            scope,
          ),
        integer: true,
        format: formatBytes,
      },
      {
        value: "data:read",
        group: "Data",
        label: "All tensors: Read",
        colour: "--read",
        values: (_, scope) =>
          trafficValues(
            scope === "global" ? data.tensors : filteredTensors(),
            "read",
            scope,
          ),
        integer: true,
        format: formatBytes,
      },
      {
        value: "data:write",
        group: "Data",
        label: "All tensors: Written",
        colour: "--write",
        values: (_, scope) =>
          trafficValues(
            scope === "global" ? data.tensors : filteredTensors(),
            "write",
            scope,
          ),
        integer: true,
        format: formatBytes,
      },
      ...["read", "write"].map((direction) => ({
        value: `tensor:${direction}`,
        group: "Data",
        label: `Selected tensor: ${direction === "read" ? "Read" : "Written"}`,
        colour: direction === "read" ? "--read" : "--write",
        values: (_, scope) =>
          trafficValues(
            state.selectedTensor ? [state.selectedTensor] : [],
            direction,
            scope,
          ),
        integer: true,
        format: formatBytes,
        context: () => state.selectedTensor?.id || "No tensor selected",
      })),
    ];
  }

  function overlayMeasures() {
    const names = new Set(
      data.pes.flatMap((pe) => Object.keys(pe.overlays || {})),
    );
    return [...names]
      .map((name) => {
        const metadata = data.overlay_metrics?.[name] || {};
        return {
          value: `metric:${name}`,
          group: "Metrics file",
          label: metadata.label || labelFromName(name),
          colour: "--metric",
          metricValue: (pe) => pe.overlays?.[name],
          format: (value) => formatUnit(value, metadata.unit || ""),
        };
      })
      .sort((left, right) => left.label.localeCompare(right.label));
  }

  const timetablePeMeasures = staticMeasures();
  const overlayPeMeasures = overlayMeasures();
  const peOverviewMeasures = [...timetablePeMeasures, ...overlayPeMeasures];
  const peChartColumnConfiguration = createOverviewColumnConfiguration(
    peOverviewMeasures.map((measure) => ({
      ...measure,
      key: measure.value,
      defaultWidth: 140,
      minWidth: 90,
    })),
    ["compute:machine-ops", "compute:compute-nodes", "data:read", "data:write"],
  );
  const peChartColumnPresets = [
    {
      key: "timetable",
      label: "Timetable data",
      keys: timetablePeMeasures.map((measure) => measure.value),
    },
    {
      key: "metrics",
      label: "Metrics overlay",
      keys: overlayPeMeasures.map((measure) => measure.value),
    },
  ];

  function appendMeasureGroup(label, measures) {
    if (!measures.length) {
      return;
    }
    const group = document.createElement("optgroup");
    group.label = label;
    for (const measure of measures) {
      group.append(
        option(measure.value, `${measure.group} · ${measure.label}`),
      );
    }
    peOverviewControls.measure.append(group);
  }

  function initializePeOverviewControls() {
    peOverviewControls.measure.replaceChildren();
    for (const group of ["Compute allocation", "Data", "Metrics file"]) {
      appendMeasureGroup(
        group,
        peOverviewMeasures.filter((measure) => measure.group === group),
      );
    }
    setPeOverviewMeasure("compute:machine-ops");
    setPeGridEncoding("colour");
    setPeChartOrder("pe-asc");
    setPeScaleMode("global");
    setPeOverviewMode("grid");
  }

  function setPeOverviewMeasure(value) {
    const measure = peOverviewMeasures.find(
      (candidate) => candidate.value === value,
    );
    if (!measure) {
      return false;
    }
    peOverviewControls.measure.value = measure.value;
    for (const action of document.querySelectorAll(
      "[data-pe-overview-measure]",
    )) {
      action.setAttribute(
        "aria-pressed",
        action.dataset.peOverviewMeasure === measure.value ? "true" : "false",
      );
    }
    return true;
  }

  function setPeOverviewMode(mode) {
    peOverviewControls.mode = mode === "chart" ? "chart" : "grid";
    peOverviewControls.chart.hidden = peOverviewControls.mode !== "chart";
    peOverviewControls.grid.hidden = peOverviewControls.mode !== "grid";
    peOverviewControls.measureControl.hidden =
      peOverviewControls.mode === "chart";
    peOverviewControls.scaleControl.hidden =
      peOverviewControls.mode === "chart";
    peOverviewControls.fixedScale.hidden =
      peOverviewControls.mode === "chart" ||
      peOverviewControls.scaleMode !== "fixed";
    peOverviewControls.gridEncodingGroup.hidden =
      peOverviewControls.mode !== "grid";
    peOverviewControls.legend.hidden = peOverviewControls.mode === "chart";
    for (const state of peOverviewControls.stateLegend.querySelectorAll(
      "[data-grid-state]",
    )) {
      state.hidden = peOverviewControls.mode !== "grid";
    }
    peOverviewControls.stateLegend.querySelector(
      "[data-filtered-state]",
    ).hidden =
      peOverviewControls.mode !== "grid" || isAllFilter(peFilterValue());
    for (const button of peOverviewControls.modes) {
      button.setAttribute(
        "aria-pressed",
        button.dataset.peOverviewMode === peOverviewControls.mode
          ? "true"
          : "false",
      );
    }
  }

  function setPeGridEncoding(encoding) {
    peOverviewControls.gridEncoding = encoding === "size" ? "size" : "colour";
    for (const button of peOverviewControls.gridEncodings) {
      button.setAttribute(
        "aria-pressed",
        button.dataset.peGridEncoding === peOverviewControls.gridEncoding
          ? "true"
          : "false",
      );
    }
  }

  function setPeChartOrder(order) {
    peOverviewControls.chartOrder = [
      "pe-asc",
      "pe-desc",
      "value-asc",
      "value-desc",
    ].includes(order)
      ? order
      : "pe-asc";
    if (peOverviewControls.chartOrder.startsWith("pe-")) {
      setPeChartSort(
        "pe",
        peOverviewControls.chartOrder.endsWith("desc") ? "desc" : "asc",
      );
    } else {
      setPeChartSort(
        selectedPeOverviewMeasure().value,
        peOverviewControls.chartOrder.endsWith("asc") ? "asc" : "desc",
      );
    }
  }

  function setPeChartSort(key, direction) {
    if (
      key !== "pe" &&
      !peOverviewMeasures.some((measure) => measure.value === key)
    ) {
      return;
    }
    peOverviewControls.chartSortKey = key;
    peOverviewControls.chartSortDirection =
      direction === "desc" ? "desc" : "asc";
  }

  function setPeScaleMode(mode) {
    const nextMode = ["global", "fixed"].includes(mode) ? mode : "filtered";
    if (nextMode === "fixed" && peOverviewControls.scaleMode !== "fixed") {
      peOverviewControls.fixedMaximum.value = currentPeOverviewMaximum();
    }
    peOverviewControls.scaleMode = nextMode;
    peOverviewControls.scale.value = peOverviewControls.scaleMode;
    peOverviewControls.fixedScale.hidden =
      peOverviewControls.mode === "chart" ||
      peOverviewControls.scaleMode !== "fixed";
  }

  function currentPeOverviewMaximum() {
    const measure = selectedPeOverviewMeasure();
    const scope =
      peOverviewControls.scaleMode === "global" ? "global" : "filtered";
    const population = data.pes.filter(
      (pe) =>
        (pe.present_in_platform || pe.present_in_timetable) &&
        (scope === "global" || filterMatches(peFilterValue(), pe.name)),
    );
    const values = peOverviewMeasureValues(population, measure, scope);
    if (measure.integer) {
      let maximum = 0n;
      for (const value of values.values()) {
        const integer = toBigInt(value);
        const magnitude = integer < 0n ? -integer : integer;
        maximum = magnitude > maximum ? magnitude : maximum;
      }
      return maximum.toString();
    }
    let maximum = 0;
    for (const value of values.values()) {
      maximum = Math.max(maximum, Math.abs(Number(value)));
    }
    return String(maximum);
  }

  function selectedPeOverviewMeasure() {
    return (
      peOverviewMeasures.find(
        (measure) => measure.value === peOverviewControls.measure.value,
      ) || peOverviewMeasures[0]
    );
  }

  function peOverviewMeasureValue(
    pe,
    measure = selectedPeOverviewMeasure(),
    scope = "filtered",
  ) {
    const rawValue = measure?.metricValue(pe, scope);
    if (rawValue === undefined || rawValue === null) {
      return null;
    }
    if (measure.integer) {
      return toBigInt(rawValue);
    }
    const value = Number(rawValue);
    return Number.isFinite(value) ? value : null;
  }

  function peOverviewMeasureValues(
    pes,
    measure = selectedPeOverviewMeasure(),
    scope = "filtered",
  ) {
    if (measure.values) {
      const values = measure.values(pes, scope);
      return new Map(
        pes.map((pe) => [
          pe.name,
          measure.integer
            ? toBigInt(values.get(pe.name))
            : Number(values.get(pe.name) || 0),
        ]),
      );
    }
    const values = new Map();
    for (const pe of pes) {
      const value = peOverviewMeasureValue(pe, measure, scope);
      if (value !== null) {
        values.set(pe.name, value);
      }
    }
    return values;
  }

  function peOverviewScaleRange(population, measure, filteredValues) {
    if (peOverviewControls.scaleMode === "fixed") {
      const maximum = Math.max(
        0.000001,
        Number(peOverviewControls.fixedMaximum.value) || 1,
      );
      const observed = [...filteredValues.values()].map(Number);
      const hasNegative = observed.some((value) => value < 0);
      const hasPositive = observed.some((value) => value > 0);
      return peOverviewValueRange(
        hasNegative && hasPositive
          ? [-maximum, maximum]
          : hasNegative
            ? [-maximum, 0]
            : [0, maximum],
      );
    }
    const valuesByPe =
      peOverviewControls.scaleMode === "global"
        ? peOverviewMeasureValues(data.pes, measure, "global")
        : filteredValues;
    return peOverviewValueRange(
      population
        .filter((pe) => valuesByPe.has(pe.name))
        .map((pe) => valuesByPe.get(pe.name)),
    );
  }

  function peOverviewValueRange(values) {
    const integer = values.some((value) => typeof value === "bigint");
    const observedMinimum = values.length
      ? values.reduce((minimum, value) =>
          metricCompare(value, minimum) < 0 ? value : minimum,
        )
      : integer
        ? 0n
        : 0;
    const observedMaximum = values.length
      ? values.reduce((maximum, value) =>
          metricCompare(value, maximum) > 0 ? value : maximum,
        )
      : integer
        ? 0n
        : 0;
    const minimum = Math.min(Number(observedMinimum), 0);
    const maximum = Math.max(Number(observedMaximum), 0);
    const span = maximum - minimum;
    const magnitude = Math.max(Math.abs(minimum), Math.abs(maximum));
    return {
      minimum,
      maximum,
      observedMinimum,
      observedMaximum,
      span: span === 0 ? 1 : span,
      magnitude: magnitude === 0 ? 1 : magnitude,
      zeroPercent:
        maximum === minimum ? 0 : ((0 - minimum) / (maximum - minimum)) * 100,
    };
  }

  function metricCompare(left, right) {
    if (typeof left === "bigint" || typeof right === "bigint") {
      return bigIntCompare(left, right);
    }
    return left - right;
  }

  function metricAverage(values) {
    if (!values.length) {
      return 0;
    }
    if (values.some((value) => typeof value === "bigint")) {
      return integerAverage(
        values.reduce((sum, value) => sum + toBigInt(value), 0n),
        values.length,
      );
    }
    return values.reduce((sum, value) => sum + value, 0) / values.length;
  }

  function renderPeOverviewLegend(population, measure, valuesByPe, scaleRange) {
    const values = population
      .filter((pe) => valuesByPe.has(pe.name))
      .map((pe) => valuesByPe.get(pe.name));
    const observedRange = peOverviewValueRange(values);
    const range = scaleRange || observedRange;
    const average = metricAverage(values);
    const context = measure.context?.();
    peOverviewControls.legend.style.setProperty(
      "--grid-colour",
      cssColour(measure.colour),
    );
    peOverviewControls.legend.style.setProperty(
      "--zero-position",
      `${range.zeroPercent}%`,
    );
    if (!values.length) {
      peOverviewControls.legend.innerHTML = `
      <div class="pe-overview-legend-title">
        <span>${escapeHtml(measure.group)}</span>
        <strong>${escapeHtml(measure.label)}</strong>
        ${context ? `<em>${escapeHtml(context)}</em>` : ""}
      </div>
      <div class="pe-overview-legend-stats"><span>No values supplied</span></div>
    `;
      return;
    }
    peOverviewControls.legend.innerHTML = `
    <div class="pe-overview-legend-title">
      <span>${escapeHtml(measure.group)}</span>
      <strong>${escapeHtml(measure.label)}</strong>
      ${context ? `<em>${escapeHtml(context)}</em>` : ""}
    </div>
    <div class="pe-overview-legend-stats"><span>Minimum ${measure.format(observedRange.observedMinimum)}</span><span>Average ${measure.format(average)}</span><span>Maximum ${measure.format(observedRange.observedMaximum)}</span></div>
    <div class="pe-overview-legend-scale" aria-hidden="true"><span>${measure.format(range.minimum)}</span><i class="${peOverviewControls.gridEncoding === "size" ? "size" : range.minimum < 0 ? (range.maximum > 0 ? "signed" : "negative") : ""}"></i><span>${measure.format(range.maximum)}</span></div>
  `;
  }

  Object.assign(App, {
    peOverviewControls,
    peOverviewMeasures,
    peChartColumnConfiguration,
    peChartColumnPresets,
    initializePeOverviewControls,
    setPeOverviewMeasure,
    setPeOverviewMode,
    setPeGridEncoding,
    setPeChartOrder,
    setPeChartSort,
    setPeScaleMode,
    selectedPeOverviewMeasure,
    computeNodeValues,
    peOverviewMeasureValues,
    peOverviewValueRange,
    peOverviewScaleRange,
    metricCompare,
    metricAverage,
    cssColour,
    renderPeOverviewLegend,
  });
})();
