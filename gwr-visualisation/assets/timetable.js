// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    data,
    fmt,
    state,
    grid,
    rowAxis,
    colAxis,
    selectedPanel,
    timetableSummary,
    layerSummary,
    layerDetail,
    computeSummary,
    allLayerNames,
    machineOpKeys,
    machineOpTypes,
    computeNodeTypes,
    emptyMachineOps,
    filterPickers,
    layerFilterValue,
    peFilterValue,
    filteredSummary,
    filteredLayers,
    aggregateLayer,
    contextSnapshot,
    filterMatches,
    machineOpsFor,
    valueFor,
    bindSelectAndFilter,
    formatCount,
    formatBytes,
    escapeHtml,
    computeNodesMarkup,
    machineOpsMarkup,
    trafficMarkup,
    dims,
    peOverviewControls,
    peChartColumnConfiguration,
    peChartColumnPresets,
    selectedPeOverviewMeasure,
    computeNodeValues,
    peOverviewMeasureValues,
    peOverviewValueRange,
    peOverviewScaleRange,
    metricCompare,
    metricAverage,
    cssColour,
    renderPeOverviewLegend,
    setPeOverviewMode,
    setPeChartSort,
    createOverviewColumnConfiguration,
    overviewColumnControlsMarkup,
    bindOverviewColumnControls,
    overviewGridTemplate,
    overviewTableMinimumWidth,
    bindOverviewColumnResizing,
    toBigInt,
    bigIntMax,
    ratioPercent,
    integerAverage,
  } = App;

  const MAX_PE_GRID_CELLS = 10_000;
  const LAYER_WINDOW_SIZE = 200;
  const layerOverviewControls = {
    sortKey: "layer",
    sortDirection: "asc",
  };
  const layerOverviewColumns = [
    {
      key: "nodes",
      label: "Compute nodes",
      colour: "var(--activity)",
      format: (value) => fmt.format(value),
      defaultWidth: 130,
      minWidth: 105,
    },
    {
      key: "ops",
      label: "Machine ops",
      colour: "var(--activity)",
      format: formatCount,
      defaultWidth: 130,
      minWidth: 105,
    },
    {
      key: "read",
      label: "Read",
      colour: "var(--read)",
      format: formatBytes,
      defaultWidth: 120,
      minWidth: 95,
    },
    {
      key: "write",
      label: "Written",
      colour: "var(--write)",
      format: formatBytes,
      defaultWidth: 120,
      minWidth: 95,
    },
    {
      key: "tensors",
      label: "Tensors",
      colour: "var(--activity)",
      format: (value) => fmt.format(value),
      defaultWidth: 100,
      minWidth: 80,
    },
    {
      key: "active-pes",
      label: "Active PEs",
      colour: "var(--activity)",
      format: (value) => fmt.format(value),
      defaultWidth: 105,
      minWidth: 85,
    },
    ...computeNodeTypes.map((type) => ({
      key: `nodes:${type.name}`,
      label: `Compute nodes: ${type.label}`,
      colour: type.colour,
      format: (value) => fmt.format(value),
      defaultWidth: 150,
      minWidth: 105,
    })),
    ...machineOpTypes.map((type) => ({
      key: `ops:${type.name}`,
      label: `Machine ops: ${type.label}`,
      colour: type.colour,
      format: formatCount,
      defaultWidth: 150,
      minWidth: 105,
    })),
  ];
  const layerOverviewColumnConfiguration = createOverviewColumnConfiguration(
    layerOverviewColumns,
    ["nodes", "ops", "read", "write", "tensors", "active-pes"],
  );
  let layerWindowStart = 0;
  let pendingLayerOverviewReveal = null;

  function setLayerOverviewSort(key, direction) {
    if (
      key !== "layer" &&
      !layerOverviewColumns.some((column) => column.key === key)
    ) {
      return;
    }
    layerOverviewControls.sortKey = key;
    layerOverviewControls.sortDirection = direction === "desc" ? "desc" : "asc";
    layerWindowStart = 0;
  }

  function peGridIsSafe(rows, cols) {
    return (
      Number.isSafeInteger(rows) &&
      Number.isSafeInteger(cols) &&
      rows > 0 &&
      cols > 0 &&
      rows <= Math.floor(MAX_PE_GRID_CELLS / cols)
    );
  }

  function renderGlobalStats() {
    const summary = filteredSummary();
    document.getElementById("stat-machine-ops").textContent = formatCount(
      summary.machineOps.total || 0,
    );
    document.getElementById("stat-compute").textContent =
      `${fmt.format(summary.computeNodes)} compute nodes`;
    document.getElementById("stat-tensors").textContent = fmt.format(
      summary.tensors.length,
    );
    document.getElementById("stat-read-bytes").textContent = formatBytes(
      summary.readBytes,
    );
    document.getElementById("stat-write-bytes").textContent = formatBytes(
      summary.writeBytes,
    );
    document.getElementById("stat-edges").textContent = fmt.format(
      summary.edges,
    );
    document.getElementById("stat-pes").textContent = fmt.format(
      summary.activePes,
    );
  }

  function selectedLayerData() {
    const layers = filteredLayers();
    if (!layers.length) {
      state.selectedLayerName = null;
      return null;
    }
    let layer = layers.find(
      (candidate) => candidate.name === state.selectedLayerName,
    );
    if (!layer) {
      layer = layers[0];
      state.selectedLayerName = layer.name;
    }
    return layer;
  }

  function renderTimetableSummary() {
    if (timetableSummary.closest("[data-view]")?.hidden) {
      return;
    }
    const summary = filteredSummary();
    const layerCount = filteredLayers().length;
    timetableSummary.innerHTML = `
    <div class="summary-metrics">
      <div><span>Layers</span><strong>${fmt.format(layerCount)}</strong></div>
      <div><span>Tensors</span><strong>${fmt.format(summary.tensors.length)}</strong></div>
      <div><span>Compute nodes</span><strong>${fmt.format(summary.computeNodes)}</strong></div>
      <div><span>Edges</span><strong>${fmt.format(summary.edges)}</strong></div>
      <div><span>Active PEs</span><strong>${fmt.format(summary.activePes)}</strong></div>
    </div>
  `;
  }

  function layerOverviewMetric(layer) {
    const aggregate = aggregateLayer(layer);
    const context = contextSnapshot(layer.name, peFilterValue());
    return {
      layer,
      nodes: aggregate.computeNodes,
      ops: toBigInt(aggregate.machineOps.total),
      read: context.readBytes,
      write: context.writeBytes,
      tensors: context.tensors.length,
      "active-pes": aggregate.activePeNames.size,
      ...Object.fromEntries(
        computeNodeTypes.map((type) => [
          `nodes:${type.name}`,
          Number(aggregate.computeNodesByOp[type.name] || 0),
        ]),
      ),
      ...Object.fromEntries(
        machineOpTypes.map((type) => [
          `ops:${type.name}`,
          toBigInt(aggregate.machineOps[type.name]),
        ]),
      ),
    };
  }

  function compareLayerOverviewMetric(left, right, key) {
    if (key === "layer") {
      return left.layer.name.localeCompare(right.layer.name, undefined, {
        numeric: true,
      });
    }
    return metricCompare(left[key], right[key]);
  }

  function sortedLayerOverviewMetrics(layers = filteredLayers()) {
    const direction = layerOverviewControls.sortDirection === "desc" ? -1 : 1;
    return layers.map(layerOverviewMetric).sort(
      (left, right) =>
        direction *
          compareLayerOverviewMetric(
            left,
            right,
            layerOverviewControls.sortKey,
          ) ||
        left.layer.name.localeCompare(right.layer.name, undefined, {
          numeric: true,
        }),
    );
  }

  function revealLayerOverviewSelection(layerName) {
    const index = sortedLayerOverviewMetrics().findIndex(
      (metric) => metric.layer.name === layerName,
    );
    if (index < 0) {
      return false;
    }
    pendingLayerOverviewReveal = layerName;
    const inWindow =
      index >= layerWindowStart && index < layerWindowStart + LAYER_WINDOW_SIZE;
    if (!inWindow) {
      layerWindowStart =
        Math.floor(index / LAYER_WINDOW_SIZE) * LAYER_WINDOW_SIZE;
    }
    const panel = layerSummary.closest("[data-view]");
    if (!inWindow || panel?.hidden) {
      return true;
    }
    scrollLayerOverviewSelectionIntoView();
    return false;
  }

  function scrollLayerOverviewSelectionIntoView() {
    if (!pendingLayerOverviewReveal) {
      return;
    }
    const row = [...layerSummary.querySelectorAll(".layer-summary-row")].find(
      (candidate) => candidate.dataset.layer === pendingLayerOverviewReveal,
    );
    const table = row?.closest(".layer-overview-table");
    if (!row || !table) {
      return;
    }
    const tableBounds = table.getBoundingClientRect();
    const headerHeight =
      table.querySelector(".layer-overview-header")?.getBoundingClientRect()
        .height || 0;
    const rowBounds = row.getBoundingClientRect();
    const visibleTop = tableBounds.top + headerHeight;
    if (rowBounds.top < visibleTop) {
      table.scrollTop -= visibleTop - rowBounds.top;
    } else if (rowBounds.bottom > tableBounds.bottom) {
      table.scrollTop += rowBounds.bottom - tableBounds.bottom;
    }
    pendingLayerOverviewReveal = null;
  }

  function layerOverviewColumnStatistics(metrics, columns) {
    return new Map(
      columns.map((column) => {
        const values = metrics.map((metric) => metric[column.key]);
        const maximum = values.reduce(
          (current, value) =>
            metricCompare(value, current) > 0 ? value : current,
          values.some((value) => typeof value === "bigint") ? 1n : 1,
        );
        return [column.key, { maximum, average: metricAverage(values) }];
      }),
    );
  }

  function layerOverviewMetricMarkup(metric, column, statistics) {
    const value = metric[column.key];
    const formatted = column.format(value);
    const width = Math.min(ratioPercent(value, statistics.maximum), 100);
    const average = Math.min(
      ratioPercent(statistics.average, statistics.maximum),
      100,
    );
    return `
      <span class="layer-overview-metric" title="${escapeHtml(`${column.label}: ${formatted}; average ${column.format(statistics.average)}`)}">
        <strong>${formatted}</strong>
        <span class="layer-overview-track" style="--metric-colour: ${column.colour}">
          <i style="width: ${width}%"></i>
          <b style="left: ${average}%" aria-hidden="true"></b>
        </span>
      </span>`;
  }

  function layerOverviewHeaderMarkup(key, label) {
    const active = layerOverviewControls.sortKey === key;
    const direction = active ? layerOverviewControls.sortDirection : "";
    const button = `<button type="button" data-layer-sort="${escapeHtml(key)}" aria-pressed="${active}" title="Sort by ${escapeHtml(label)}"><span>${escapeHtml(label)}</span><i aria-hidden="true">${direction === "asc" ? "↑" : direction === "desc" ? "↓" : ""}</i></button>`;
    if (key === "layer") {
      return button;
    }
    return `<span class="overview-column-header" data-column-key="${escapeHtml(key)}">${button}<span class="overview-column-resize" data-column-resize="${escapeHtml(key)}" role="separator" aria-orientation="vertical" aria-label="Resize ${escapeHtml(label)} column" tabindex="0" title="Drag to resize ${escapeHtml(label)}; double-click to distribute columns evenly"></span></span>`;
  }

  function renderLayerSummary() {
    if (layerSummary.closest("[data-view]")?.hidden) {
      return;
    }
    const layers = filteredLayers();
    if (!layers.length) {
      layerSummary.innerHTML = `<p>No graph layers found.</p>`;
      return;
    }
    selectedLayerData();
    const columns = layerOverviewColumnConfiguration.visible();
    const metrics = sortedLayerOverviewMetrics(layers);
    App.bindOverviewKeyboard(
      layerSummary,
      metrics.map((metric) => metric.layer.name),
      () => state.selectedLayerName,
      (name) => App.selectGraphLayer(name),
    );
    const statistics = layerOverviewColumnStatistics(metrics, columns);
    layerSummary.innerHTML = "";
    layerWindowStart = Math.min(
      layerWindowStart,
      Math.max(0, metrics.length - LAYER_WINDOW_SIZE),
    );
    const visibleMetrics = metrics.slice(
      layerWindowStart,
      layerWindowStart + LAYER_WINDOW_SIZE,
    );
    if (metrics.length > LAYER_WINDOW_SIZE) {
      const navigator = document.createElement("label");
      navigator.className = "layer-window-navigator";
      navigator.innerHTML = `<span>Displaying ${fmt.format(layerWindowStart + 1)}-${fmt.format(layerWindowStart + visibleMetrics.length)} of ${fmt.format(metrics.length)} layers</span><input type="range" min="0" max="${Math.max(0, metrics.length - LAYER_WINDOW_SIZE)}" step="${LAYER_WINDOW_SIZE}" value="${layerWindowStart}" aria-label="First displayed layer">`;
      navigator.querySelector("input").addEventListener("input", (event) => {
        layerWindowStart = Number(event.target.value);
        renderLayerSummary();
      });
      layerSummary.append(navigator);
    } else {
      const status = document.createElement("p");
      status.className = "display-status";
      status.textContent = `Displaying ${fmt.format(metrics.length)} layers · Bars use filtered maximum; markers show filtered average`;
      layerSummary.append(status);
    }

    const identityWidth = 180;
    const tableWidth = overviewTableMinimumWidth(
      layerOverviewColumnConfiguration,
      identityWidth,
    );
    const gridTemplate = overviewGridTemplate(
      layerOverviewColumnConfiguration,
      identityWidth,
    );
    layerSummary.insertAdjacentHTML(
      "beforeend",
      `${overviewColumnControlsMarkup(layerOverviewColumnConfiguration)}
       <div class="layer-overview-table overview-table" style="--overview-grid-template: ${gridTemplate}; --overview-table-width: ${tableWidth}px">
         <div class="layer-overview-header">
           ${layerOverviewHeaderMarkup("layer", "Layer")}
           ${columns.map((column) => layerOverviewHeaderMarkup(column.key, column.label)).join("")}
         </div>
         <div class="layer-summary-list"></div>
       </div>`,
    );
    const list = layerSummary.querySelector(".layer-summary-list");
    const fragment = document.createDocumentFragment();

    for (const metric of visibleMetrics) {
      const { layer } = metric;
      const row = document.createElement("button");
      row.type = "button";
      row.className = "layer-summary-row";
      row.dataset.layer = layer.name;
      if (layer.name === state.selectedLayerName) {
        row.classList.add("selected");
      }
      row.setAttribute(
        "aria-pressed",
        layer.name === state.selectedLayerName ? "true" : "false",
      );
      row.setAttribute(
        "aria-label",
        `${layer.name}: ${columns
          .map(
            (column) => `${column.label} ${column.format(metric[column.key])}`,
          )
          .join(", ")}`,
      );
      row.innerHTML = `
        <span class="layer-overview-name" title="${escapeHtml(layer.name)}">${escapeHtml(layer.name)}</span>
        ${columns
          .map((column) =>
            layerOverviewMetricMarkup(
              metric,
              column,
              statistics.get(column.key),
            ),
          )
          .join("")}`;
      bindSelectAndFilter(
        row,
        () => App.selectLayer(layer.name),
        filterPickers.layers,
        layer.name,
      );
      fragment.append(row);
    }
    list.append(fragment);

    bindOverviewColumnControls(
      layerSummary,
      layerOverviewColumnConfiguration,
      [],
      () => {
        if (
          layerOverviewControls.sortKey !== "layer" &&
          !layerOverviewColumnConfiguration
            .visible()
            .some((column) => column.key === layerOverviewControls.sortKey)
        ) {
          setLayerOverviewSort("layer", "asc");
        }
        renderLayerSummary();
        App.workspaceChanged?.();
      },
    );
    bindOverviewColumnResizing(
      layerSummary,
      layerOverviewColumnConfiguration,
      identityWidth,
      () => {
        renderLayerSummary();
        App.workspaceChanged?.();
      },
    );
    for (const header of layerSummary.querySelectorAll("[data-layer-sort]")) {
      header.addEventListener("click", () => {
        const sortKey = header.dataset.layerSort;
        const sortDirection =
          layerOverviewControls.sortKey === sortKey &&
          layerOverviewControls.sortDirection === "desc"
            ? "asc"
            : "desc";
        setLayerOverviewSort(sortKey, sortDirection);
        renderLayerSummary();
        App.workspaceChanged?.();
      });
    }
    scrollLayerOverviewSelectionIntoView();
  }

  function renderLayerDetail() {
    if (layerDetail.closest("[data-view]")?.hidden) {
      return;
    }
    const layer = selectedLayerData();
    if (!layer) {
      layerDetail.innerHTML = `<p>No graph layer selected.</p>`;
      return;
    }
    const aggregate = aggregateLayer(layer);
    const layerContext = contextSnapshot(layer.name, peFilterValue());
    const layerOps = aggregate.machineOps;
    const computeNodes = aggregate.computeNodes;
    const computeNodesByOp = aggregate.computeNodesByOp;
    layerDetail.innerHTML = `
    <div class="layer-detail-heading"><h3>${escapeHtml(layer.name)}</h3><span>${fmt.format(aggregate.activePeNames.size)} PEs</span></div>
    ${trafficMarkup({ read: layerContext.readBytes, write: layerContext.writeBytes })}
    ${computeNodesMarkup(computeNodes, computeNodesByOp)}
    ${machineOpsMarkup(layerOps)}
  `;
  }

  function computePopulation() {
    return data.pes.filter(
      (pe) =>
        (pe.present_in_platform || pe.present_in_timetable) &&
        filterMatches(peFilterValue(), pe.name),
    );
  }

  function aggregatePeAcrossLayers(
    peName,
    layerSelection = layerFilterValue(),
  ) {
    return filteredLayers(layerSelection).reduce(
      (total, layer) => {
        const layerPe = (layer.pes || []).find((pe) => pe.name === peName);
        total.computeNodes += Number(layerPe?.compute_nodes || 0);
        for (const [op, count] of Object.entries(layerPe?.by_op || {})) {
          total.computeNodesByOp[op] =
            Number(total.computeNodesByOp[op] || 0) + Number(count || 0);
        }
        return total;
      },
      { computeNodes: 0, computeNodesByOp: {} },
    );
  }

  function renderComputeSummary() {
    if (computeSummary.closest("[data-view]")?.hidden) {
      return;
    }
    const population = computePopulation();
    const values = population.map((pe) => valueFor(pe));
    const total = values.reduce((sum, value) => sum + value, 0n);
    const maximum = values.reduce((max, value) => bigIntMax(max, value), 0n);
    const average = integerAverage(total, population.length);
    const allocated = values.filter((value) => value > 0n).length;
    const imbalance = average ? Number(maximum) / Number(average) : 0;
    const selectedLayers = filteredLayers();
    const layer =
      selectedLayers.length === allLayerNames.length
        ? "All layers"
        : `${fmt.format(selectedLayers.length)} layers`;
    const machineOps = population.reduce((totals, pe) => {
      const ops = machineOpsFor(pe) || {};
      for (const key of ["total", ...machineOpKeys]) {
        totals[key] += toBigInt(ops[key]);
      }
      return totals;
    }, emptyMachineOps());
    const summary = filteredSummary();

    computeSummary.innerHTML = `
    <div class="compute-summary-context"><strong>Machine ops</strong><span>${escapeHtml(layer)}</span></div>
    <div class="summary-metrics">
      <div><span>Total</span><strong>${formatCount(total)}</strong></div>
      <div><span>Average per PE</span><strong>${formatCount(average)}</strong></div>
      <div><span>Maximum</span><strong>${formatCount(maximum)}</strong></div>
      <div><span>Max / average</span><strong>${imbalance.toFixed(2)}×</strong></div>
      <div><span>Allocated PEs</span><strong>${fmt.format(allocated)} / ${fmt.format(population.length)}</strong></div>
    </div>
    ${computeNodesMarkup(summary.computeNodes, summary.computeNodesByOp)}
    ${machineOpsMarkup(machineOps)}
  `;
  }

  function renderPeChart() {
    const population = computePopulation();
    const columns = peChartColumnConfiguration.visible();
    const statistics = new Map(
      columns.map((measure) => {
        const values = peOverviewMeasureValues(data.pes, measure);
        const availableValues = population
          .filter((pe) => values.has(pe.name))
          .map((pe) => values.get(pe.name));
        const range = peOverviewValueRange(availableValues);
        return [
          measure.key,
          {
            values,
            range,
            average: metricAverage(availableValues),
          },
        ];
      }),
    );
    const rows = [...population].sort((left, right) => {
      const direction =
        peOverviewControls.chartSortDirection === "desc" ? -1 : 1;
      if (peOverviewControls.chartSortKey === "pe") {
        return direction * peIdOrder(left, right);
      }
      const values = statistics.get(peOverviewControls.chartSortKey)?.values;
      const leftAvailable = values?.has(left.name) || false;
      const rightAvailable = values?.has(right.name) || false;
      if (leftAvailable !== rightAvailable) {
        return leftAvailable ? -1 : 1;
      }
      return (
        direction *
          metricCompare(
            values?.get(left.name) ?? 0,
            values?.get(right.name) ?? 0,
          ) || peIdOrder(left, right)
      );
    });
    const identityWidth = 150;
    App.bindOverviewKeyboard(
      peOverviewControls.chart,
      rows.map((pe) => pe.name),
      () => state.selectedPe?.name,
      (name) => App.selectPe(App.pesByName.get(name)),
    );
    const tableWidth = overviewTableMinimumWidth(
      peChartColumnConfiguration,
      identityWidth,
    );
    const gridTemplate = overviewGridTemplate(
      peChartColumnConfiguration,
      identityWidth,
    );

    peOverviewControls.chart.innerHTML = `
      <div class="pe-overview-chart-status"><span>Displaying ${rows.length} of ${data.pes.length} PEs</span><span>Bars use per-column filtered ranges; markers show filtered averages</span></div>
      ${columns.some((column) => column.key.startsWith("tensor:")) ? `<p>Selected tensor: ${escapeHtml(state.selectedTensor?.id || "None")}</p>` : ""}
      ${overviewColumnControlsMarkup(peChartColumnConfiguration, peChartColumnPresets)}
      <div class="pe-overview-table overview-table" style="--overview-grid-template: ${gridTemplate}; --overview-table-width: ${tableWidth}px">
        <div class="pe-overview-chart-header">
          ${peChartHeaderMarkup("pe", "PE ID")}
          ${columns.map((column) => peChartHeaderMarkup(column.key, column.label)).join("")}
        </div>
        <div class="pe-overview-chart-list"></div>
      </div>`;

    const list = peOverviewControls.chart.querySelector(
      ".pe-overview-chart-list",
    );
    for (const pe of rows) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "pe-overview-chart-row";
      if (pe === state.selectedPe) {
        row.classList.add("selected");
      }
      row.setAttribute(
        "aria-label",
        `${pe.name}: ${columns
          .map((column) => {
            const value = statistics.get(column.key).values.get(pe.name);
            return `${column.label} ${value === undefined ? "unavailable" : column.format(value)}`;
          })
          .join(", ")}`,
      );
      row.innerHTML = `
        <span class="pe-overview-chart-name" title="${escapeHtml(pe.name)}">${escapeHtml(pe.name)}</span>
        ${columns
          .map((column) =>
            peChartMetricMarkup(column, statistics.get(column.key), pe.name),
          )
          .join("")}`;
      bindSelectAndFilter(
        row,
        () => {
          App.selectPe(pe);
        },
        filterPickers.pes,
        pe.name,
      );
      list.append(row);
    }

    bindOverviewColumnControls(
      peOverviewControls.chart,
      peChartColumnConfiguration,
      peChartColumnPresets,
      () => {
        if (
          peOverviewControls.chartSortKey !== "pe" &&
          !peChartColumnConfiguration
            .visible()
            .some((column) => column.key === peOverviewControls.chartSortKey)
        ) {
          setPeChartSort("pe", "asc");
        }
        renderPeChart();
        App.workspaceChanged?.();
      },
    );
    bindOverviewColumnResizing(
      peOverviewControls.chart,
      peChartColumnConfiguration,
      identityWidth,
      () => {
        renderPeChart();
        App.workspaceChanged?.();
      },
    );
    for (const button of peOverviewControls.chart.querySelectorAll(
      "[data-pe-chart-sort]",
    )) {
      button.addEventListener("click", () => {
        const key = button.dataset.peChartSort;
        const direction =
          peOverviewControls.chartSortKey === key &&
          peOverviewControls.chartSortDirection === "desc"
            ? "asc"
            : "desc";
        setPeChartSort(key, direction);
        renderPeChart();
        App.workspaceChanged?.();
      });
    }
  }

  function peChartMetricMarkup(measure, statistics, peName) {
    const value = statistics.values.get(peName);
    if (value === undefined) {
      return `<span class="pe-overview-chart-metric unavailable">Unavailable</span>`;
    }
    const numericValue = Number(value);
    const { range, average } = statistics;
    const barStart = Math.min(numericValue, 0);
    const left = ((barStart - range.minimum) / range.span) * 100;
    const width = Math.min((Math.abs(numericValue) / range.span) * 100, 100);
    const averagePosition = Math.min(
      Math.max(((Number(average) - range.minimum) / range.span) * 100, 0),
      100,
    );
    const formatted = measure.format(value);
    return `
      <span class="pe-overview-chart-metric" title="${escapeHtml(`${measure.label}: ${formatted}; average ${measure.format(average)}`)}">
        <strong>${formatted}</strong>
        <span class="pe-overview-chart-track" style="--overview-colour: ${cssColour(measure.colour)}; --zero-position: ${range.zeroPercent}%">
          <i class="pe-overview-chart-fill${numericValue < 0 ? " negative" : ""}" style="left: ${left}%; width: ${width}%"></i>
          <b style="left: ${averagePosition}%" aria-hidden="true"></b>
        </span>
      </span>`;
  }

  function peChartHeaderMarkup(key, label) {
    const active = peOverviewControls.chartSortKey === key;
    const direction = active ? peOverviewControls.chartSortDirection : "";
    const button = `<button type="button" data-pe-chart-sort="${escapeHtml(key)}" aria-pressed="${active}" title="Sort by ${escapeHtml(label)}"><span>${escapeHtml(label)}</span><i aria-hidden="true">${direction === "asc" ? "↑" : direction === "desc" ? "↓" : ""}</i></button>`;
    if (key === "pe") {
      return button;
    }
    return `<span class="overview-column-header" data-column-key="${escapeHtml(key)}">${button}<span class="overview-column-resize" data-column-resize="${escapeHtml(key)}" role="separator" aria-orientation="vertical" aria-label="Resize ${escapeHtml(label)} column" tabindex="0" title="Drag to resize ${escapeHtml(label)}; double-click to distribute columns evenly"></span></span>`;
  }

  function peIdOrder(left, right) {
    return left.name.localeCompare(right.name, undefined, {
      numeric: true,
      sensitivity: "base",
    });
  }

  function renderGrid() {
    if (grid.closest("[data-view]")?.hidden) {
      return;
    }
    const [rows, cols] = dims();
    if (!peGridIsSafe(rows, cols)) {
      setPeOverviewMode("chart");
      renderPeChart();
      return;
    }
    const byCoord = new Map();
    for (const pe of data.pes) {
      const key = `${pe.row},${pe.col}`;
      const pes = byCoord.get(key) || [];
      pes.push(pe);
      byCoord.set(key, pes);
    }
    const population = computePopulation();
    const measure = selectedPeOverviewMeasure();
    const valuesByPe = peOverviewMeasureValues(data.pes, measure);
    const computeNodesByPe =
      measure.value === "compute:compute-nodes"
        ? valuesByPe
        : computeNodeValues(data.pes);
    const range = peOverviewScaleRange(population, measure, valuesByPe);
    grid.style.gridTemplateColumns = `repeat(${cols}, clamp(14px, 3.8vw, 34px))`;
    colAxis.style.gridTemplateColumns = `repeat(${cols}, clamp(14px, 3.8vw, 34px))`;
    rowAxis.style.gridTemplateRows = `repeat(${rows}, clamp(14px, 3.8vw, 34px))`;
    grid.innerHTML = "";
    rowAxis.innerHTML = "";
    colAxis.innerHTML = "";
    renderPeOverviewLegend(population, measure, valuesByPe, range);

    for (let row = 0; row < rows; row++) {
      const label = document.createElement("span");
      label.textContent = row;
      rowAxis.append(label);
    }
    for (let col = 0; col < cols; col++) {
      const label = document.createElement("span");
      label.textContent = col;
      colAxis.append(label);
    }

    for (let row = 0; row < rows; row++) {
      for (let col = 0; col < cols; col++) {
        const pes = byCoord.get(`${row},${col}`) || [];
        const cell = document.createElement("div");
        cell.className = "pe-cell";
        if (!pes.length) {
          const button = document.createElement("button");
          button.type = "button";
          button.className = "pe empty";
          button.disabled = true;
          button.setAttribute(
            "aria-label",
            `No processing element at ${row}, ${col}`,
          );
          cell.append(button);
        } else {
          const columns = Math.ceil(Math.sqrt(pes.length));
          cell.style.gridTemplateColumns = `repeat(${columns}, minmax(0, 1fr))`;
          if (pes.length > 1) {
            cell.classList.add("multiple");
          }
          for (const pe of pes) {
            const button = document.createElement("button");
            button.type = "button";
            button.className = "pe";
            const matchesFilter = filterMatches(peFilterValue(), pe.name);
            const hasValue = valuesByPe.has(pe.name);
            const value = hasValue ? valuesByPe.get(pe.name) : 0;
            const numericValue = Number(value);
            button.title = hasValue
              ? `${pe.name}: ${measure.format(value)} ${measure.label}`
              : `${pe.name}: no value supplied`;
            if (!matchesFilter) {
              button.title = `${pe.name}: filtered out`;
            }
            const normalized =
              matchesFilter && hasValue
                ? Math.abs(numericValue) / range.magnitude
                : 0;
            const intensity = Math.round(10 + Math.sqrt(normalized) * 90);
            const size = Math.sqrt(normalized) * 100;
            button.style.setProperty(
              "--grid-colour",
              numericValue < 0 ? "var(--write)" : cssColour(measure.colour),
            );
            button.style.setProperty(
              "--intensity",
              `${matchesFilter && hasValue && numericValue !== 0 ? intensity : 0}%`,
            );
            button.style.setProperty(
              "--size",
              `${matchesFilter && hasValue && numericValue !== 0 ? size : 0}%`,
            );
            button.style.setProperty(
              "--platform",
              pe.present_in_platform ? "16%" : "0%",
            );
            button.setAttribute(
              "aria-label",
              matchesFilter
                ? `${pe.name}, ${hasValue ? measure.format(value) : "no value supplied"} ${measure.group} ${measure.label}, ${fmt.format(computeNodesByPe.get(pe.name) || 0)} compute nodes`
                : `${pe.name}, filtered out`,
            );
            button.classList.toggle("unavailable", !hasValue);
            button.classList.toggle(
              "size-encoded",
              peOverviewControls.gridEncoding === "size",
            );
            if (pe === state.selectedPe) {
              button.classList.add("selected");
            }
            if (!matchesFilter) {
              button.classList.add("filtered-out");
            }
            bindSelectAndFilter(
              button,
              () => {
                App.selectPe(pe);
              },
              filterPickers.pes,
              pe.name,
            );
            cell.append(button);
          }
        }
        grid.append(cell);
      }
    }
  }

  function renderPeOverview() {
    if (grid.closest("[data-view]")?.hidden) {
      return;
    }
    setPeOverviewMode(peOverviewControls.mode);
    if (peOverviewControls.mode === "chart") {
      renderPeChart();
    } else {
      renderGrid();
    }
  }

  function renderSelected() {
    if (selectedPanel.closest("[data-view]")?.hidden) {
      return;
    }
    if (!state.selectedPe) {
      selectedPanel.textContent = "No processing elements found.";
      return;
    }
    const overlayPills = Object.entries(state.selectedPe.overlays || {})
      .map(([name, value]) => {
        const meta = data.overlay_metrics?.[name];
        const label = escapeHtml(meta?.label || name);
        const unit = meta?.unit ? ` ${escapeHtml(meta.unit)}` : "";
        return `<span class="pill">${label}: ${fmt.format(value)}${unit}</span>`;
      })
      .join("");
    const platform = state.selectedPe.platform_config
      ? `<p>Platform: ${escapeHtml(state.selectedPe.platform_config.memory_map)}, active requests ${state.selectedPe.platform_config.num_active_requests ?? "n/a"}, LSU ${state.selectedPe.platform_config.lsu_access_bytes ?? "n/a"} bytes</p>`
      : "<p>Platform: no platform PE entry</p>";
    const peTraffic = (pe) => {
      const context = contextSnapshot(layerFilterValue(), pe.name);
      return { read: context.readBytes, write: context.writeBytes };
    };
    const selectedTraffic = peTraffic(state.selectedPe);
    const trafficPopulation = computePopulation();
    const trafficMaximum = trafficPopulation.reduce((maximum, pe) => {
      const traffic = peTraffic(pe);
      return bigIntMax(maximum, bigIntMax(traffic.read, traffic.write));
    }, 1n);
    const readBytes = selectedTraffic.read;
    const writeBytes = selectedTraffic.write;
    const trafficTotals = trafficPopulation.reduce(
      (totals, pe) => {
        const traffic = peTraffic(pe);
        totals.read += traffic.read;
        totals.write += traffic.write;
        return totals;
      },
      { read: 0n, write: 0n },
    );
    const averageRead = integerAverage(
      trafficTotals.read,
      trafficPopulation.length,
    );
    const averageWrite = integerAverage(
      trafficTotals.write,
      trafficPopulation.length,
    );
    const populationValues = trafficPopulation.map((pe) => valueFor(pe));
    const maxCompute = populationValues.reduce(
      (maximum, value) => bigIntMax(maximum, value),
      1n,
    );
    const averageCompute = integerAverage(
      populationValues.reduce((sum, value) => sum + value, 0n),
      populationValues.length,
    );
    const ops = machineOpsFor(state.selectedPe) || {};
    const peAggregate = aggregatePeAcrossLayers(state.selectedPe.name);
    const computeNodes = peAggregate.computeNodes;
    const computeNodesByOp = peAggregate.computeNodesByOp;
    const populationNodeCounts = trafficPopulation.map(
      (pe) => aggregatePeAcrossLayers(pe.name).computeNodes,
    );
    const maximumNodes = populationNodeCounts.reduce(
      (maximum, value) => Math.max(maximum, value),
      1,
    );
    const averageNodes = Math.floor(
      populationNodeCounts.reduce((sum, value) => sum + value, 0) /
        Math.max(populationNodeCounts.length, 1),
    );

    selectedPanel.innerHTML = `
    <h2>${escapeHtml(state.selectedPe.name)}</h2>
    <p>Row ${state.selectedPe.row}, column ${state.selectedPe.col}</p>
    ${platform}
    ${computeNodesMarkup(computeNodes, computeNodesByOp, {
      maximum: maximumNodes,
      marker: ratioPercent(averageNodes, maximumNodes),
      caption: `Compared with filtered PEs; average ${formatCount(averageNodes)}`,
    })}
    ${machineOpsMarkup(ops, {
      maximum: maxCompute,
      marker: ratioPercent(averageCompute, maxCompute),
      caption: `Compared with filtered PEs; average ${formatCount(averageCompute)}`,
    })}
    ${trafficMarkup({
      read: readBytes,
      write: writeBytes,
      maximum: trafficMaximum,
      averageRead,
      averageWrite,
      caption: `Compared with filtered PEs; averages Read ${formatBytes(averageRead)}, Written ${formatBytes(averageWrite)}`,
    })}
    <div class="overlay-list">${overlayPills}</div>
  `;
  }

  Object.assign(App, {
    layerOverviewControls,
    layerOverviewColumnConfiguration,
    setLayerOverviewSort,
    revealLayerOverviewSelection,
    renderGlobalStats,
    renderTimetableSummary,
    renderLayerSummary,
    renderLayerDetail,
    renderComputeSummary,
    renderPeOverview,
    renderSelected,
  });
})();
