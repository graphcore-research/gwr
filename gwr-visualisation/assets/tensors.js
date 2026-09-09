// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    data,
    fmt,
    state,
    tensorOverview,
    tensorMemory,
    skipMemoryGaps,
    memoryVisualMode,
    selectedTensorPanel,
    memoryLayoutCache,
    tensorOverviewMetricsCache,
    filterPickers,
    filteredTensors,
    tensorTraffic,
    scaleTensorToMemory,
    cacheKey,
    isAllFilter,
    layerFilterValue,
    peFilterValue,
    memoryFilterValue,
    tensorFilterValue,
    filterMatches,
    bindSelectAndFilter,
    markSelectionElement,
    toBigInt,
    bigIntMax,
    bigIntCompare,
    bigIntToNumber,
    ratioPercent,
    clipTensorToMemory,
    formatBytes,
    formatHex,
    escapeHtml,
    createOverviewColumnConfiguration,
    overviewColumnControlsMarkup,
    bindOverviewColumnControls,
    overviewGridTemplate,
    overviewTableMinimumWidth,
    bindOverviewColumnResizing,
  } = App;

  const TENSOR_OVERVIEW_PAGE_SIZES = [100, 200, 500];
  const tensorOverviewControls = {
    sortKey: "read-ratio",
    sortDirection: "desc",
    page: 0,
    pageSize: 200,
  };
  let pendingTensorOverviewReveal = null;
  let pendingTensorMemoryReveal = null;
  const tensorOverviewColumns = [
    {
      key: "size",
      label: "Size",
      colour: "var(--activity)",
      format: formatBytes,
      defaultWidth: 110,
    },
    {
      key: "read",
      label: "Read",
      colour: "var(--read)",
      format: formatBytes,
      defaultWidth: 110,
    },
    {
      key: "write",
      label: "Written",
      colour: "var(--write)",
      format: formatBytes,
      defaultWidth: 110,
    },
    {
      key: "read-ratio",
      label: "Read %",
      colour: "var(--read)",
      format: (value) => `${Number(value).toFixed(1)}%`,
      defaultWidth: 105,
    },
    {
      key: "write-ratio",
      label: "Written %",
      colour: "var(--write)",
      format: (value) => `${Number(value).toFixed(1)}%`,
      defaultWidth: 105,
    },
    {
      key: "dtype",
      label: "Data type",
      defaultWidth: 90,
    },
    {
      key: "reading-pes",
      label: "Reading PEs",
      colour: "var(--read)",
      format: (value) => fmt.format(value),
      defaultWidth: 110,
    },
    {
      key: "writing-pes",
      label: "Writing PEs",
      colour: "var(--write)",
      format: (value) => fmt.format(value),
      defaultWidth: 110,
    },
  ];
  const tensorOverviewColumnConfiguration = createOverviewColumnConfiguration(
    tensorOverviewColumns,
    tensorOverviewColumns.map((column) => column.key),
  );

  function tensorMetrics(tensor) {
    const traffic = tensorTraffic(tensor);
    const size = scaleTensorToMemory(tensor, tensor.num_bytes);
    const read = scaleTensorToMemory(tensor, traffic.readBytes);
    const write = scaleTensorToMemory(tensor, traffic.writtenBytes);
    const divisor = bigIntToNumber(bigIntMax(size, 1n));
    return {
      tensor,
      size,
      read,
      write,
      "read-ratio": (bigIntToNumber(read) / divisor) * 100,
      "write-ratio": (bigIntToNumber(write) / divisor) * 100,
      dtype: tensor.dtype,
      "reading-pes": traffic.reads.length,
      "writing-pes": traffic.writes.length,
    };
  }

  function tensorOverviewMetrics() {
    const key = cacheKey(
      layerFilterValue(),
      peFilterValue(),
      memoryFilterValue(),
      tensorFilterValue(),
    );
    if (!tensorOverviewMetricsCache.has(key)) {
      tensorOverviewMetricsCache.set(key, filteredTensors().map(tensorMetrics));
    }
    return tensorOverviewMetricsCache.get(key);
  }

  function compareTensorMetric(left, right, key) {
    const leftValue = key === "tensor" ? left.tensor.id : left[key];
    const rightValue = key === "tensor" ? right.tensor.id : right[key];
    if (typeof leftValue === "bigint" || typeof rightValue === "bigint") {
      return bigIntCompare(toBigInt(leftValue), toBigInt(rightValue));
    }
    if (typeof leftValue === "number" && typeof rightValue === "number") {
      return leftValue - rightValue;
    }
    return String(leftValue).localeCompare(String(rightValue), undefined, {
      numeric: true,
    });
  }

  function sortedTensorOverviewMetrics(metrics) {
    const direction = tensorOverviewControls.sortDirection === "asc" ? 1 : -1;
    return [...metrics].sort(
      (left, right) =>
        direction *
          compareTensorMetric(left, right, tensorOverviewControls.sortKey) ||
        left.tensor.id.localeCompare(right.tensor.id, undefined, {
          numeric: true,
        }),
    );
  }

  function tensorColumnStatistics(metrics) {
    return Object.fromEntries(
      tensorOverviewColumnConfiguration
        .visible()
        .filter((column) => column.format)
        .map((column) => {
          const values = metrics.map((metric) => metric[column.key]);
          if (values.some((value) => typeof value === "bigint")) {
            const total = values.reduce(
              (sum, value) => sum + toBigInt(value),
              0n,
            );
            return [
              column.key,
              {
                maximum: values.reduce(
                  (maximum, value) => bigIntMax(maximum, toBigInt(value)),
                  1n,
                ),
                average: total / BigInt(Math.max(values.length, 1)),
              },
            ];
          }
          const total = values.reduce(
            (sum, value) => sum + Number(value || 0),
            0,
          );
          return [
            column.key,
            {
              maximum: values.reduce(
                (maximum, value) => Math.max(maximum, Number(value || 0)),
                1,
              ),
              average: total / Math.max(values.length, 1),
            },
          ];
        }),
    );
  }

  function tensorMetricPercent(value, maximum) {
    if (typeof value === "bigint" || typeof maximum === "bigint") {
      return ratioPercent(toBigInt(value), bigIntMax(toBigInt(maximum), 1n));
    }
    return (Number(value || 0) / Math.max(Number(maximum || 0), 1)) * 100;
  }

  function tensorMetricMarkup(metric, column, statistics) {
    const value = metric[column.key];
    const formatted = column.format(value);
    const width = Math.min(tensorMetricPercent(value, statistics.maximum), 100);
    const average = Math.min(
      tensorMetricPercent(statistics.average, statistics.maximum),
      100,
    );
    return `
      <span class="tensor-overview-metric" title="${escapeHtml(`${column.label}: ${formatted}; average ${column.format(statistics.average)}`)}">
        <strong>${formatted}</strong>
        <span class="tensor-overview-track" style="--metric-colour: ${column.colour}">
          <i style="width: ${width}%"></i>
          <b style="left: ${average}%" aria-hidden="true"></b>
        </span>
      </span>`;
  }

  function tensorSortHeaderMarkup(key, label, title = label) {
    const selected = tensorOverviewControls.sortKey === key;
    const direction = selected ? tensorOverviewControls.sortDirection : "";
    const button = `<button type="button" data-tensor-sort="${escapeHtml(key)}" aria-pressed="${selected}" title="${escapeHtml(`Sort by ${title}`)}"><span>${escapeHtml(label)}</span><i aria-hidden="true">${direction === "asc" ? "↑" : direction === "desc" ? "↓" : ""}</i></button>`;
    if (key === "tensor") {
      return button;
    }
    return `<span class="overview-column-header" data-column-key="${escapeHtml(key)}">${button}<span class="overview-column-resize" data-column-resize="${escapeHtml(key)}" role="separator" aria-orientation="vertical" aria-label="Resize ${escapeHtml(label)} column" tabindex="0" title="Drag to resize ${escapeHtml(label)}; double-click to distribute columns evenly"></span></span>`;
  }

  function setTensorOverviewSort(key, direction) {
    const validKeys = new Set([
      "tensor",
      ...tensorOverviewColumns.map((column) => column.key),
    ]);
    if (!validKeys.has(key)) {
      return;
    }
    tensorOverviewControls.sortKey = key;
    tensorOverviewControls.sortDirection = direction === "asc" ? "asc" : "desc";
    tensorOverviewControls.page = 0;
  }

  function setTensorOverviewPageSize(value) {
    const pageSize = Number(value);
    tensorOverviewControls.pageSize = TENSOR_OVERVIEW_PAGE_SIZES.includes(
      pageSize,
    )
      ? pageSize
      : 200;
    tensorOverviewControls.page = 0;
  }

  function resetTensorOverviewPage() {
    tensorOverviewControls.page = 0;
  }

  function revealTensorOverviewSelection(tensorId) {
    const index = sortedTensorOverviewMetrics(
      tensorOverviewMetrics(),
    ).findIndex((metric) => metric.tensor.id === tensorId);
    if (index < 0) {
      return false;
    }
    pendingTensorOverviewReveal = tensorId;
    const page = Math.floor(index / tensorOverviewControls.pageSize);
    tensorOverviewControls.page = page;
    // Graph navigation can change selection again before the next animation
    // frame. Re-render first so the requested row is present when it is
    // scrolled into view.
    return true;
  }

  function scrollTensorOverviewSelectionIntoView() {
    if (!pendingTensorOverviewReveal) {
      return;
    }
    const row = [
      ...tensorOverview.querySelectorAll(".tensor-overview-row"),
    ].find(
      (candidate) => candidate.dataset.tensor === pendingTensorOverviewReveal,
    );
    const table = row?.closest(".tensor-overview-table");
    if (!row || !table) {
      return;
    }
    const tableBounds = table.getBoundingClientRect();
    const headerHeight =
      table.querySelector(".tensor-overview-header")?.getBoundingClientRect()
        .height || 0;
    const rowBounds = row.getBoundingClientRect();
    const visibleTop = tableBounds.top + headerHeight;
    table.scrollTop += rowBounds.top - visibleTop;
    pendingTensorOverviewReveal = null;
  }

  function renderTensorOverview() {
    const panel = tensorOverview.closest("[data-view]");
    if (panel?.hidden) {
      return;
    }
    if (pendingTensorOverviewReveal)
      revealTensorOverviewSelection(pendingTensorOverviewReveal);
    const metrics = tensorOverviewMetrics();
    if (!metrics.length) {
      tensorOverview.textContent = "No tensor nodes match the current filters.";
      return;
    }

    const sortedMetrics = sortedTensorOverviewMetrics(metrics);
    App.bindOverviewKeyboard(
      tensorOverview,
      sortedMetrics.map((metric) => metric.tensor.id),
      () => state.selectedTensor?.id,
      (id) => App.selectTensor(data.tensors.find((tensor) => tensor.id === id)),
    );
    const statistics = tensorColumnStatistics(metrics);
    const pageCount = Math.ceil(
      sortedMetrics.length / tensorOverviewControls.pageSize,
    );
    tensorOverviewControls.page = Math.min(
      tensorOverviewControls.page,
      Math.max(pageCount - 1, 0),
    );
    const start = tensorOverviewControls.page * tensorOverviewControls.pageSize;
    const visibleMetrics = sortedMetrics.slice(
      start,
      start + tensorOverviewControls.pageSize,
    );
    const visibleColumns = tensorOverviewColumnConfiguration.visible();
    const identityWidth = 240;
    const tableWidth = overviewTableMinimumWidth(
      tensorOverviewColumnConfiguration,
      identityWidth,
    );
    const gridTemplate = overviewGridTemplate(
      tensorOverviewColumnConfiguration,
      identityWidth,
    );

    tensorOverview.innerHTML = `
      <div class="tensor-overview-status">
        <span>Displaying ${fmt.format(start + 1)}-${fmt.format(start + visibleMetrics.length)} of ${fmt.format(sortedMetrics.length)} tensors</span>
        <span>Bars use filtered maximum; markers show filtered average</span>
        <label>Rows <select aria-label="Tensor overview rows per page">${TENSOR_OVERVIEW_PAGE_SIZES.map((value) => `<option value="${value}"${value === tensorOverviewControls.pageSize ? " selected" : ""}>${value}</option>`).join("")}</select></label>
        <button type="button" data-tensor-page="previous"${tensorOverviewControls.page === 0 ? " disabled" : ""}>Previous</button>
        <button type="button" data-tensor-page="next"${tensorOverviewControls.page >= pageCount - 1 ? " disabled" : ""}>Next</button>
      </div>
      ${overviewColumnControlsMarkup(tensorOverviewColumnConfiguration)}
      <div class="tensor-overview-table overview-table" style="--overview-grid-template: ${gridTemplate}; --overview-table-width: ${tableWidth}px">
        <div class="tensor-overview-header">
          ${tensorSortHeaderMarkup("tensor", "Tensor", "tensor name")}
          ${visibleColumns
            .map((column) => tensorSortHeaderMarkup(column.key, column.label))
            .join("")}
        </div>
        <div class="tensor-overview-rows"></div>
      </div>`;

    const rows = tensorOverview.querySelector(".tensor-overview-rows");
    const fragment = document.createDocumentFragment();
    for (const metric of visibleMetrics) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "tensor-overview-row";
      row.dataset.tensor = metric.tensor.id;
      row.classList.toggle("selected", metric.tensor === state.selectedTensor);
      row.setAttribute("aria-pressed", metric.tensor === state.selectedTensor);
      row.setAttribute(
        "aria-label",
        `${metric.tensor.id}, ${formatBytes(metric.size)}, ${formatBytes(metric.read)} read, ${formatBytes(metric.write)} written, ${metric["read-ratio"].toFixed(1)}% read, ${metric["write-ratio"].toFixed(1)}% written, ${metric.dtype}, ${fmt.format(metric["reading-pes"])} reading PEs, ${fmt.format(metric["writing-pes"])} writing PEs`,
      );
      row.innerHTML = `
        <span class="tensor-overview-name" title="${escapeHtml(metric.tensor.id)}">${escapeHtml(metric.tensor.id)}</span>
        ${visibleColumns
          .map((column) =>
            column.format
              ? tensorMetricMarkup(metric, column, statistics[column.key])
              : `<span class="tensor-overview-dtype">${escapeHtml(metric[column.key])}</span>`,
          )
          .join("")}`;
      bindSelectAndFilter(
        row,
        () => {
          App.selectTensor(metric.tensor);
        },
        filterPickers.tensors,
        metric.tensor.id,
      );
      fragment.append(row);
    }
    rows.append(fragment);

    bindOverviewColumnControls(
      tensorOverview,
      tensorOverviewColumnConfiguration,
      [],
      () => {
        if (
          tensorOverviewControls.sortKey !== "tensor" &&
          !tensorOverviewColumnConfiguration
            .visible()
            .some((column) => column.key === tensorOverviewControls.sortKey)
        ) {
          const firstColumn = tensorOverviewColumnConfiguration.visible()[0];
          setTensorOverviewSort(
            firstColumn?.key || "tensor",
            firstColumn ? "desc" : "asc",
          );
        }
        renderTensorOverview();
        App.workspaceChanged?.();
      },
    );
    bindOverviewColumnResizing(
      tensorOverview,
      tensorOverviewColumnConfiguration,
      identityWidth,
      () => {
        renderTensorOverview();
        App.workspaceChanged?.();
      },
    );

    for (const header of tensorOverview.querySelectorAll(
      "[data-tensor-sort]",
    )) {
      header.addEventListener("click", () => {
        const key = header.dataset.tensorSort;
        const direction =
          tensorOverviewControls.sortKey === key &&
          tensorOverviewControls.sortDirection === "desc"
            ? "asc"
            : "desc";
        setTensorOverviewSort(key, direction);
        renderTensorOverview();
        App.workspaceChanged?.();
      });
    }
    tensorOverview
      .querySelector("select")
      .addEventListener("change", (event) => {
        setTensorOverviewPageSize(event.target.value);
        renderTensorOverview();
        App.workspaceChanged?.();
      });
    for (const button of tensorOverview.querySelectorAll(
      "[data-tensor-page]",
    )) {
      button.addEventListener("click", () => {
        tensorOverviewControls.page +=
          button.dataset.tensorPage === "next" ? 1 : -1;
        renderTensorOverview();
      });
    }
    scrollTensorOverviewSelectionIntoView();
  }

  function revealTensorMemorySelection(tensorId) {
    pendingTensorMemoryReveal = tensorId;
  }

  function scrollTensorMemorySelectionIntoView() {
    if (!pendingTensorMemoryReveal) return;
    const row = [...tensorMemory.querySelectorAll(".memory-tensor-row")].find(
      (candidate) =>
        candidate.dataset.selectionId === pendingTensorMemoryReveal,
    );
    if (!row) return;
    const id = pendingTensorMemoryReveal;
    const align = () => {
      if (
        !row.isConnected ||
        !tensorMemory.offsetParent ||
        state.selectedTensor?.id !== id
      )
        return;
      const bounds = tensorMemory.getBoundingClientRect();
      const rowBounds = row.getBoundingClientRect();
      tensorMemory.scrollTop += rowBounds.top - bounds.top;
    };
    align();
    // Off-screen rows replace their estimated heights after becoming visible.
    requestAnimationFrame(() => requestAnimationFrame(align));
    pendingTensorMemoryReveal = null;
  }

  function renderTensorMemory() {
    const panel = tensorMemory.closest("[data-view]");
    if (panel?.hidden) {
      return;
    }
    const tensors = filteredTensors();
    if (!tensors.length) {
      tensorMemory.textContent = "No tensor nodes found.";
      return;
    }

    const memoryKey = cacheKey(
      skipMemoryGaps.checked,
      layerFilterValue(),
      peFilterValue(),
      memoryFilterValue(),
      tensorFilterValue(),
      state.selectedTensor?.id || null,
      state.selectedMemoryName || null,
      memoryVisualMode.value,
    );
    if (
      state.renderedTensorMemoryKey === memoryKey &&
      tensorMemory.querySelector(".memory-regions")
    ) {
      scrollTensorMemorySelectionIntoView();
      return;
    }
    state.renderedTensorMemoryKey = memoryKey;

    const regions = getMemoryLayout(skipMemoryGaps.checked, tensors);
    const allocatedBytes = regions.reduce(
      (sum, region) => sum + region.allocated,
      0n,
    );
    const previousScrollTop = tensorMemory.scrollTop;
    tensorMemory.innerHTML = "";

    const container = document.createElement("div");
    container.className = `memory-regions memory-mode-${memoryVisualMode.value}`;
    const selectedMemoryTensorIds = new Set(
      (data.memory?.platform_memories || []).find(
        (memory) => memory.name === state.selectedMemoryName,
      )?.tensors || [],
    );
    const fragment = document.createDocumentFragment();

    for (const region of regions) {
      if (region.gapBefore > 0) {
        const gap = document.createElement("div");
        gap.className = "memory-gap-row";
        gap.textContent = `${formatBytes(region.gapBefore)} unused`;
        fragment.append(gap);
      }

      const row = document.createElement("section");
      row.className = "memory-region";
      const header = document.createElement("div");
      header.className = "memory-region-header";
      header.innerHTML = `
      <strong>${formatHex(region.start)}-${formatHex(region.end)}</strong>
      <span>${fmt.format(region.tensors.length)} tensors</span>
      <span>${formatBytes(region.allocated)} allocated</span>
      <span>${formatBytes(region.span)} span</span>
    `;

      const tensorRows = document.createElement("div");
      tensorRows.className = "memory-region-tensors";
      for (const tensorLayout of region.tensors) {
        const tensor = tensorLayout.tensor || tensorLayout;
        const tensorRow = document.createElement("div");
        tensorRow.className = "memory-tensor-row";
        markSelectionElement(tensorRow, filterPickers.tensors, tensor.id);
        if (tensor === state.selectedTensor) {
          tensorRow.classList.add("selected");
        }
        tensorRow.classList.toggle(
          "related-to-memory",
          selectedMemoryTensorIds.has(tensor.id),
        );

        const label = document.createElement("button");
        label.type = "button";
        label.className = "memory-tensor-label";
        label.textContent = tensor.id;
        label.title = tensor.id;
        bindSelectAndFilter(
          label,
          () => {
            App.selectTensor(tensor);
          },
          filterPickers.tensors,
          tensor.id,
        );

        const track = document.createElement("div");
        track.className = "memory-tensor-track";
        const block = document.createElement("button");
        const addr = toBigInt(tensorLayout.addr);
        const bytes = bigIntMax(toBigInt(tensorLayout.num_bytes), 1n);
        const traffic = tensorTraffic(tensor);
        const left =
          (bigIntToNumber(addr - region.start) / bigIntToNumber(region.span)) *
          100;
        const width = Math.max(
          (bigIntToNumber(bytes) / bigIntToNumber(region.span)) * 100,
          0.35,
        );
        block.type = "button";
        block.className = "memory-tensor-block";
        if (tensor === state.selectedTensor) {
          block.classList.add("selected");
        }
        block.style.left = `${left}%`;
        block.style.width = `${Math.min(width, 100 - left)}%`;
        block.style.setProperty("--write-share", `${traffic.writeShare}%`);
        block.style.setProperty("--read-share", `${traffic.readShare}%`);
        block.title = memoryTensorTitle(tensor);
        block.setAttribute("aria-label", memoryTensorTitle(tensor));
        block.innerHTML = `
        <span class="memory-tensor-fill read"></span>
        <span class="memory-tensor-fill write"></span>
      `;
        bindSelectAndFilter(
          block,
          () => {
            App.selectTensor(tensor);
          },
          filterPickers.tensors,
          tensor.id,
        );
        track.append(block);

        const size = document.createElement("span");
        size.className = "memory-tensor-size";
        size.textContent = `W ${Math.round(traffic.writeRatio * 100)}% / R ${Math.round(traffic.readRatio * 100)}%`;
        size.title = formatBytes(bytes);
        tensorRow.append(label, track, size);
        tensorRows.append(tensorRow);
      }

      row.append(header, tensorRows);
      fragment.append(row);
    }
    container.append(fragment);

    const legend = document.createElement("div");
    legend.className = "memory-legend";
    legend.innerHTML = `
    <span><i class="tensor"></i>allocation outline</span>
    <span><i class="read"></i>read %</span>
    <span><i class="write"></i>written %</span>
    <span><i class="gap"></i>unused gap${skipMemoryGaps.checked ? " (collapsed between regions)" : ""}</span>
    <strong>Displaying ${fmt.format(tensors.length)} of ${fmt.format(data.tensors.length)} tensors · ${formatBytes(allocatedBytes)} allocated</strong>
  `;

    tensorMemory.append(container, legend);
    tensorMemory.scrollTop = previousScrollTop;
    scrollTensorMemorySelectionIntoView();
  }

  function getMemoryLayout(skipGaps, tensors = filteredTensors()) {
    const key = cacheKey(
      skipGaps,
      layerFilterValue(),
      peFilterValue(),
      memoryFilterValue(),
      tensorFilterValue(),
    );
    if (!memoryLayoutCache.has(key)) {
      memoryLayoutCache.set(key, buildMemoryLayout(skipGaps, tensors));
    }
    return memoryLayoutCache.get(key);
  }

  function buildMemoryLayout(skipGaps, tensors = filteredTensors()) {
    const memorySelection = memoryFilterValue();
    const selectedMemories = (data.memory?.platform_memories || []).filter(
      (memory) => filterMatches(memorySelection, memory.name),
    );
    const layouts = isAllFilter(memorySelection)
      ? tensors
      : tensors.flatMap((tensor) =>
          selectedMemories
            .map((memory) => clipTensorToMemory(tensor, memory))
            .filter(Boolean),
        );
    const sorted = [...layouts].sort(
      (a, b) => bigIntCompare(a.addr, b.addr) || a.id.localeCompare(b.id),
    );
    return buildTensorRegions(sorted, skipGaps);
  }

  function buildTensorRegions(tensors, skipGaps) {
    const largestTensorBytes = tensors.reduce(
      (largest, tensor) => bigIntMax(largest, toBigInt(tensor.num_bytes)),
      1n,
    );
    const totalTensorBytes = tensors.reduce(
      (sum, tensor) => sum + toBigInt(tensor.num_bytes),
      0n,
    );
    const largeGapThreshold = skipGaps
      ? bigIntMax(largestTensorBytes, bigIntMax(totalTensorBytes / 64n, 4096n))
      : null;
    const regions = [];
    let current = null;

    for (const tensor of tensors) {
      const addr = toBigInt(tensor.addr);
      const bytes = bigIntMax(toBigInt(tensor.num_bytes), 1n);
      const end = addr + bytes;
      const gap = current ? bigIntMax(addr - current.end, 0n) : 0n;

      if (!current || (largeGapThreshold !== null && gap > largeGapThreshold)) {
        current = {
          start: addr,
          end,
          gapBefore: regions.length ? gap : 0n,
          allocated: bytes,
          tensors: [tensor],
        };
        regions.push(current);
      } else {
        current.end = bigIntMax(current.end, end);
        current.allocated += bytes;
        current.tensors.push(tensor);
      }
    }

    for (const region of regions) {
      region.span = bigIntMax(region.end - region.start, 1n);
    }
    return regions;
  }

  function memoryTensorTitle(tensor) {
    const consumerPeCount = (tensor.consumption_by_pe || []).length;
    const traffic = tensorTraffic(tensor);
    return `${tensor.id}: ${formatHex(tensor.addr)}, ${formatBytes(tensor.num_bytes)}, read ${traffic.readRatio.toFixed(2)}x, written ${traffic.writeRatio.toFixed(2)}x, consumed by ${fmt.format(consumerPeCount)} PEs`;
  }

  function renderSelectedTensor() {
    if (selectedTensorPanel.closest("[data-view]")?.hidden) {
      return;
    }
    if (!state.selectedTensor) {
      selectedTensorPanel.textContent = "No tensor selected.";
      return;
    }

    const metrics = tensorMetrics(state.selectedTensor);
    const maxBytes = bigIntMax(
      bigIntMax(metrics.size, metrics.write),
      bigIntMax(metrics.read, 1n),
    );
    const bars = [
      ["Tensor size", metrics.size, "", "size"],
      ["Read", metrics.read, `${metrics["read-ratio"].toFixed(1)}%`, "read"],
      [
        "Written",
        metrics.write,
        `${metrics["write-ratio"].toFixed(1)}%`,
        "write",
      ],
    ]
      .map(
        ([label, bytes, percentage, mode]) => `
      <div class="tensor-byte-row">
        <span>${label}</span>
        <div class="tensor-byte-track"><div class="tensor-byte-fill ${mode}" style="width: ${ratioPercent(bytes, maxBytes)}%"></div></div>
        <strong>${formatBytes(bytes)}</strong>
        <em>${percentage}</em>
      </div>
    `,
      )
      .join("");

    selectedTensorPanel.innerHTML = `
    <h2>${escapeHtml(state.selectedTensor.id)}</h2>
    <p>${formatHex(state.selectedTensor.addr)} · [${escapeHtml(state.selectedTensor.shape.join(" × "))}]</p>
    <div class="summary-metrics tensor-detail-metrics">
      <div><span>Data type</span><strong>${escapeHtml(metrics.dtype)}</strong></div>
      <div><span>Reading PEs</span><strong>${fmt.format(metrics["reading-pes"])}</strong></div>
      <div><span>Writing PEs</span><strong>${fmt.format(metrics["writing-pes"])}</strong></div>
    </div>
    <div class="tensor-byte-summary">${bars}</div>
  `;
  }

  Object.assign(App, {
    tensorOverviewControls,
    tensorOverviewColumnConfiguration,
    setTensorOverviewSort,
    setTensorOverviewPageSize,
    resetTensorOverviewPage,
    revealTensorOverviewSelection,
    revealTensorMemorySelection,
    renderTensorOverview,
    renderTensorMemory,
    getMemoryLayout,
    buildTensorRegions,
    memoryTensorTitle,
    renderSelectedTensor,
  });
})();
