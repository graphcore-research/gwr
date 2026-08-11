// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    data,
    state,
    machineOpTypes,
    machineOpsFor,
    filteredLayers,
    filteredTensors,
    tensorTraffic,
    scaleTensorToMemory,
    option,
    labelFromName,
    formatCount,
    formatBytes,
    escapeHtml,
    toBigInt,
    bigIntCompare,
    integerAverage,
  } = App;

  const peOverviewControls = {
    measure: document.getElementById("pe-overview-measure"),
    modes: [...document.querySelectorAll("[data-pe-overview-mode]")],
    chart: document.getElementById("pe-overview-chart"),
    grid: document.getElementById("pe-overview-grid"),
    legend: document.getElementById("pe-overview-legend"),
    mode: "grid",
  };
  const overlayNumber = new Intl.NumberFormat("en", {
    maximumFractionDigits: 2,
  });

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

  function computeNodeValues(pes) {
    const values = new Map(pes.map((pe) => [pe.name, 0]));
    for (const layer of filteredLayers()) {
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

  function trafficValues(tensors, direction) {
    const values = new Map();
    for (const tensor of tensors) {
      const traffic = tensorTraffic(tensor);
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
            scaleTensorToMemory(tensor, connection.bytes),
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
        metricValue: (pe) => toBigInt(machineOpsFor(pe)?.total),
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
        colour: "--activity",
        metricValue: (pe) => toBigInt(machineOpsFor(pe)?.[machineOp.name]),
        integer: true,
        format: formatCount,
      })),
      {
        value: "data:total",
        group: "Data",
        label: "Total",
        colour: "--activity",
        values: () => trafficValues(filteredTensors(), "total"),
        integer: true,
        format: formatBytes,
      },
      {
        value: "data:read",
        group: "Data",
        label: "Read",
        colour: "--read",
        values: () => trafficValues(filteredTensors(), "read"),
        integer: true,
        format: formatBytes,
      },
      {
        value: "data:write",
        group: "Data",
        label: "Written",
        colour: "--write",
        values: () => trafficValues(filteredTensors(), "write"),
        integer: true,
        format: formatBytes,
      },
      {
        value: "tensor:read",
        group: "Selected tensor",
        label: "Read bytes",
        colour: "--read",
        values: () =>
          trafficValues(
            state.selectedTensor ? [state.selectedTensor] : [],
            "read",
          ),
        integer: true,
        format: formatBytes,
        context: () => state.selectedTensor?.id || "No tensor selected",
      },
      {
        value: "tensor:write",
        group: "Selected tensor",
        label: "Written bytes",
        colour: "--write",
        values: () =>
          trafficValues(
            state.selectedTensor ? [state.selectedTensor] : [],
            "write",
          ),
        integer: true,
        format: formatBytes,
        context: () => state.selectedTensor?.id || "No tensor selected",
      },
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

  const peOverviewMeasures = [...staticMeasures(), ...overlayMeasures()];

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
    for (const group of [
      "Compute allocation",
      "Data",
      "Selected tensor",
      "Metrics file",
    ]) {
      appendMeasureGroup(
        group,
        peOverviewMeasures.filter((measure) => measure.group === group),
      );
    }
    peOverviewControls.measure.value = "compute:machine-ops";
    setPeOverviewMode("grid");
  }

  function setPeOverviewMode(mode) {
    peOverviewControls.mode = mode === "chart" ? "chart" : "grid";
    peOverviewControls.chart.hidden = peOverviewControls.mode !== "chart";
    peOverviewControls.grid.hidden = peOverviewControls.mode !== "grid";
    for (const button of peOverviewControls.modes) {
      button.setAttribute(
        "aria-pressed",
        button.dataset.peOverviewMode === peOverviewControls.mode
          ? "true"
          : "false",
      );
    }
  }

  function selectedPeOverviewMeasure() {
    return (
      peOverviewMeasures.find(
        (measure) => measure.value === peOverviewControls.measure.value,
      ) || peOverviewMeasures[0]
    );
  }

  function peOverviewMeasureValue(pe, measure = selectedPeOverviewMeasure()) {
    const rawValue = measure?.metricValue(pe);
    if (rawValue === undefined || rawValue === null) {
      return null;
    }
    if (measure.integer) {
      return toBigInt(rawValue);
    }
    const value = Number(rawValue);
    return Number.isFinite(value) ? value : null;
  }

  function peOverviewMeasureValues(pes, measure = selectedPeOverviewMeasure()) {
    if (measure.values) {
      const values = measure.values(pes);
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
      const value = peOverviewMeasureValue(pe, measure);
      if (value !== null) {
        values.set(pe.name, value);
      }
    }
    return values;
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

  function renderPeOverviewLegend(population, measure, valuesByPe) {
    const values = population
      .filter((pe) => valuesByPe.has(pe.name))
      .map((pe) => valuesByPe.get(pe.name));
    const range = peOverviewValueRange(values);
    const average = metricAverage(values);
    const context = measure.context?.();
    peOverviewControls.legend.style.setProperty(
      "--grid-colour",
      `var(${measure.colour})`,
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
    <div class="pe-overview-legend-stats"><span>Minimum ${measure.format(range.observedMinimum)}</span><span>Average ${measure.format(average)}</span><span>Maximum ${measure.format(range.observedMaximum)}</span></div>
    <div class="pe-overview-legend-scale" aria-hidden="true"><span>${measure.format(range.minimum)}</span><i class="${range.minimum < 0 ? (range.maximum > 0 ? "signed" : "negative") : ""}"></i><span>${measure.format(range.maximum)}</span></div>
  `;
  }

  Object.assign(App, {
    peOverviewControls,
    initializePeOverviewControls,
    setPeOverviewMode,
    selectedPeOverviewMeasure,
    computeNodeValues,
    peOverviewMeasureValues,
    peOverviewValueRange,
    metricCompare,
    metricAverage,
    renderPeOverviewLegend,
  });
})();
