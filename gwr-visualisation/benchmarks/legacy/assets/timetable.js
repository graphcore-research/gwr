// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    data,
    fmt,
    state,
    pesByName,
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
    emptyMachineOps,
    filterPickers,
    layerFilterValue,
    peFilterValue,
    filteredSummary,
    filteredLayers,
    aggregateLayer,
    contextSnapshot,
    contextTensorCount,
    filterMatches,
    machineOpsFor,
    valueFor,
    bindSelectAndFilter,
    formatCount,
    formatBytes,
    escapeHtml,
    metricBreakdownMarkup,
    computeNodesMarkup,
    machineOpsMarkup,
    comparisonMaxima,
    comparisonBarsMarkup,
    dims,
    peOverviewControls,
    selectedPeOverviewMeasure,
    computeNodeValues,
    peOverviewMeasureValues,
    peOverviewValueRange,
    metricCompare,
    metricAverage,
    renderPeOverviewLegend,
    setPeOverviewMode,
    toBigInt,
    bigIntMax,
    ratioPercent,
    integerAverage,
  } = App;

  const MAX_PE_GRID_CELLS = 10_000;

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
      summary.dataEdges,
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
    const machineOps = summary.machineOps || {};
    const layerCount = filteredLayers().length;
    timetableSummary.innerHTML = `
    ${metricBreakdownMarkup(
      "Layers",
      layerCount,
      [
        ["Read", summary.readBytes, formatBytes],
        ["Written", summary.writeBytes, formatBytes],
      ],
      true,
    )}
    ${computeNodesMarkup(summary.computeNodes, summary.computeNodesByOp)}
    ${machineOpsMarkup(machineOps)}
  `;
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
    const metrics = layers.map((layer) => {
      const aggregate = aggregateLayer(layer);
      const context = contextSnapshot(layer.name, peFilterValue());
      return {
        layer,
        nodes: aggregate.computeNodes,
        ops: toBigInt(aggregate.machineOps.total),
        tensors: context.tensors,
        read: context.readBytes,
        write: context.writeBytes,
        activePes: aggregate.activePeNames.size,
      };
    });
    const maxima = comparisonMaxima(metrics);
    layerSummary.innerHTML = "";
    const list = document.createElement("div");
    list.className = "layer-summary-list";

    for (const metric of metrics) {
      const { layer, nodes, ops, read, write } = metric;
      const row = document.createElement("button");
      row.type = "button";
      row.className = "layer-summary-row comparison-row";
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
        `${layer.name}: ${fmt.format(nodes)} compute nodes, ${fmt.format(ops)} machine ops, ${formatBytes(read)} read, ${formatBytes(write)} written`,
      );
      row.innerHTML = `
      <div class="comparison-heading">
        <strong>${escapeHtml(layer.name)}</strong>
        <span>${fmt.format(metric.activePes)} PEs · ${fmt.format(metric.tensors.length)} tensors</span>
      </div>
      ${comparisonBarsMarkup(metric, maxima)}
    `;
      bindSelectAndFilter(
        row,
        () => {
          state.selectedLayerName = layer.name;
          App.selectionChanged("layer");
        },
        filterPickers.layers,
        layer.name,
      );
      list.append(row);
    }
    layerSummary.append(list);
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
    const peMetrics = [...(layer.pes || [])]
      .filter((pe) => filterMatches(peFilterValue(), pe.name))
      .sort((left, right) => {
        const leftPe = pesByName.get(left.name) || {};
        const rightPe = pesByName.get(right.name) || {};
        return (
          Number(leftPe.row || 0) - Number(rightPe.row || 0) ||
          Number(leftPe.col || 0) - Number(rightPe.col || 0) ||
          left.name.localeCompare(right.name)
        );
      })
      .map((pe) => {
        const context = contextSnapshot(layer.name, pe.name);
        return {
          pe,
          nodes: Number(pe.compute_nodes || 0),
          ops: toBigInt(pe.machine_ops?.total),
          tensorCount: contextTensorCount(context),
          read: context.readBytes,
          write: context.writeBytes,
        };
      });
    const peMaxima = comparisonMaxima(peMetrics);

    layerDetail.innerHTML = `
    <div class="layer-detail-heading"><h3>${escapeHtml(layer.name)}</h3><span>${fmt.format(aggregate.activePeNames.size)} PEs</span></div>
    <div class="layer-detail-metrics">
      <div><span>Read</span><strong>${formatBytes(layerContext.readBytes)}</strong></div>
      <div><span>Written</span><strong>${formatBytes(layerContext.writeBytes)}</strong></div>
    </div>
    ${computeNodesMarkup(computeNodes, computeNodesByOp)}
    ${machineOpsMarkup(layerOps)}
    <div class="layer-pe-summary-list"></div>
  `;

    const list = layerDetail.querySelector(".layer-pe-summary-list");
    if (!peMetrics.length) {
      list.innerHTML = `<p>No processing elements in this layer.</p>`;
      return;
    }

    for (const metric of peMetrics) {
      const pe = pesByName.get(metric.pe.name);
      const selected = pe?.name === state.selectedPe?.name;
      const row = document.createElement("button");
      row.type = "button";
      row.className = "layer-pe-summary-row comparison-row";
      row.classList.toggle("selected", selected);
      row.setAttribute("aria-pressed", selected ? "true" : "false");
      row.setAttribute(
        "aria-label",
        `${metric.pe.name}: ${fmt.format(metric.nodes)} compute nodes, ${fmt.format(metric.ops)} machine ops, ${formatBytes(metric.read)} read, ${formatBytes(metric.write)} written`,
      );
      row.innerHTML = `
      <div class="comparison-heading">
        <strong>${escapeHtml(metric.pe.name)}</strong>
        <span>${fmt.format(metric.tensorCount)} tensors</span>
      </div>
      ${comparisonBarsMarkup(metric, peMaxima)}
    `;
      bindSelectAndFilter(
        row,
        () => {
          if (pe) {
            state.selectedPe = pe;
            App.selectionChanged("pe");
          }
        },
        filterPickers.pes,
        metric.pe.name,
      );
      list.append(row);
    }
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

    computeSummary.innerHTML = `
    <div class="compute-summary-context"><strong>Machine ops</strong><span>${escapeHtml(layer)}</span></div>
    <div class="compute-summary-metrics">
      <div><span>Total</span><strong>${formatCount(total)}</strong></div>
      <div><span>Average per PE</span><strong>${formatCount(average)}</strong></div>
      <div><span>Maximum</span><strong>${formatCount(maximum)}</strong></div>
      <div><span>Max / average</span><strong>${imbalance.toFixed(2)}×</strong></div>
      <div><span>Allocated PEs</span><strong>${fmt.format(allocated)} / ${fmt.format(population.length)}</strong></div>
    </div>
    ${machineOpsMarkup(machineOps)}
  `;
  }

  function renderPeChart() {
    const population = computePopulation();
    const measure = selectedPeOverviewMeasure();
    const values = peOverviewMeasureValues(data.pes, measure);
    const rows = population
      .filter((pe) => values.has(pe.name))
      .map((pe) => ({ pe, value: values.get(pe.name) }))
      .sort(
        (left, right) =>
          metricCompare(right.value, left.value) ||
          left.pe.row - right.pe.row ||
          left.pe.col - right.pe.col,
      );
    const range = peOverviewValueRange(rows.map((row) => row.value));
    const average = metricAverage(rows.map((row) => row.value));
    const averagePercent =
      ((Number(average) - range.minimum) / range.span) * 100;
    peOverviewControls.chart.innerHTML = "";
    peOverviewControls.chart.style.setProperty(
      "--overview-colour",
      `var(${measure.colour})`,
    );

    const legend = document.createElement("div");
    const context = measure.context?.();
    legend.className = "pe-overview-chart-legend";
    legend.innerHTML = rows.length
      ? `<span>${escapeHtml(measure.group)} · ${escapeHtml(measure.label)}${context ? ` · ${escapeHtml(context)}` : ""}</span><span>Minimum ${measure.format(range.observedMinimum)}</span><span>Average ${measure.format(average)}</span><span>Maximum ${measure.format(range.observedMaximum)}</span>`
      : `<span>${escapeHtml(measure.group)} · ${escapeHtml(measure.label)}${context ? ` · ${escapeHtml(context)}` : ""}</span><span>No values supplied</span>`;
    peOverviewControls.chart.append(legend);

    if (!rows.length) {
      return;
    }

    const list = document.createElement("div");
    list.className = "pe-overview-chart-list";
    for (const { pe, value } of rows) {
      const numericValue = Number(value);
      const barStart = Math.min(numericValue, 0);
      const barLeft = ((barStart - range.minimum) / range.span) * 100;
      const barWidth = (Math.abs(numericValue) / range.span) * 100;
      const row = document.createElement("button");
      row.type = "button";
      row.className = "pe-overview-chart-row";
      if (pe === state.selectedPe) {
        row.classList.add("selected");
      }
      row.setAttribute(
        "aria-label",
        `${pe.name}, ${measure.format(value)} ${measure.group} ${measure.label}`,
      );
      row.innerHTML = `
      <span>${escapeHtml(pe.name)}</span>
      <div class="pe-overview-chart-track">
        <div class="pe-overview-chart-fill${value < 0 ? " negative" : ""}" style="left: ${barLeft}%; width: ${barWidth}%"></div>
        <i style="left: ${averagePercent}%" aria-hidden="true"></i>
      </div>
      <strong>${measure.format(value)}</strong>
    `;
      bindSelectAndFilter(
        row,
        () => {
          state.selectedPe = pe;
          App.selectionChanged("pe");
        },
        filterPickers.pes,
        pe.name,
      );
      list.append(row);
    }
    peOverviewControls.chart.append(list);
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
    const range = peOverviewValueRange(
      population.map((pe) => valuesByPe.get(pe.name) ?? 0),
    );
    grid.style.gridTemplateColumns = `repeat(${cols}, clamp(14px, 3.8vw, 34px))`;
    colAxis.style.gridTemplateColumns = `repeat(${cols}, clamp(14px, 3.8vw, 34px))`;
    rowAxis.style.gridTemplateRows = `repeat(${rows}, clamp(14px, 3.8vw, 34px))`;
    grid.innerHTML = "";
    rowAxis.innerHTML = "";
    colAxis.innerHTML = "";
    renderPeOverviewLegend(population, measure, valuesByPe);

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
            button.title = hasValue ? pe.name : `${pe.name}: no value supplied`;
            const normalized =
              matchesFilter && hasValue
                ? Math.abs(numericValue) / range.magnitude
                : 0;
            const intensity = Math.round(10 + Math.sqrt(normalized) * 90);
            button.style.setProperty(
              "--grid-colour",
              `var(${numericValue < 0 ? "--write" : measure.colour})`,
            );
            button.style.setProperty(
              "--intensity",
              `${matchesFilter && hasValue && numericValue !== 0 ? intensity : 0}%`,
            );
            button.style.setProperty(
              "--platform",
              pe.present_in_platform ? "16%" : "0%",
            );
            button.setAttribute(
              "aria-label",
              `${pe.name}, ${hasValue ? measure.format(value) : "no value supplied"} ${measure.group} ${measure.label}, ${fmt.format(computeNodesByPe.get(pe.name) || 0)} compute nodes`,
            );
            button.classList.toggle("unavailable", !hasValue);
            if (pe === state.selectedPe) {
              button.classList.add("selected");
            }
            if (!matchesFilter) {
              button.classList.add("filtered-out");
            }
            bindSelectAndFilter(
              button,
              () => {
                state.selectedPe = pe;
                App.selectionChanged("pe");
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
    const readPercent = ratioPercent(readBytes, trafficMaximum);
    const writePercent = ratioPercent(writeBytes, trafficMaximum);
    const selectedValue = valueFor(state.selectedPe);
    const populationValues = computePopulation().map((pe) => valueFor(pe));
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

    selectedPanel.innerHTML = `
    <h2>${escapeHtml(state.selectedPe.name)}</h2>
    <p>Row ${state.selectedPe.row}, column ${state.selectedPe.col}</p>
    ${platform}
    <div class="selected-compute" aria-label="Static compute allocation">
      <div><span>Machine ops</span><strong>${formatCount(selectedValue)}</strong></div>
      <div class="selected-compute-track">
        <div style="width: ${ratioPercent(selectedValue, maxCompute)}%"></div>
        <i style="left: ${ratioPercent(averageCompute, maxCompute)}%" aria-hidden="true"></i>
      </div>
      <p>${ratioPercent(selectedValue, maxCompute).toFixed(1)}% of maximum · average ${formatCount(averageCompute)}</p>
      ${computeNodesMarkup(computeNodes, computeNodesByOp)}
      ${machineOpsMarkup(ops)}
    </div>
    <div class="pe-traffic" aria-label="Tensor traffic">
      <span>Read</span>
      <div class="traffic-track read"><div style="width: ${readPercent}%"></div></div>
      <strong>${formatBytes(readBytes)} <em>${readPercent.toFixed(1)}%</em></strong>
      <span>Written</span>
      <div class="traffic-track write"><div style="width: ${writePercent}%"></div></div>
      <strong>${formatBytes(writeBytes)} <em>${writePercent.toFixed(1)}%</em></strong>
    </div>
    <div class="overlay-list">${overlayPills}</div>
  `;
  }

  Object.assign(App, {
    renderGlobalStats,
    renderTimetableSummary,
    renderLayerSummary,
    renderLayerDetail,
    renderComputeSummary,
    renderPeOverview,
    renderSelected,
  });
})();
