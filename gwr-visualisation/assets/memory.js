// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    data,
    fmt,
    state,
    tensorsById,
    memorySummary,
    memoriesOverview,
    memoryDetail,
    memoryMetricsCache,
    skipMemoryGaps,
    memoryVisualMode,
    filterPickers,
    filteredTensors,
    tensorTraffic,
    scaleTensorToMemory,
    cacheKey,
    filterMatches,
    layerFilterValue,
    peFilterValue,
    memoryFilterValue,
    tensorFilterValue,
    bindSelectAndFilter,
    markSelectionElement,
    toBigInt,
    bigIntMax,
    bigIntCompare,
    bigIntToNumber,
    ratioPercent,
    integerAverage,
    addressRange,
    overlapBytes,
    clipTensorToMemory,
    formatBytes,
    formatHex,
    escapeHtml,
    metricStripMarkup,
    trafficMarkup,
    buildTensorRegions,
    memoryTensorTitle,
    createOverviewColumnConfiguration,
    overviewColumnControlsMarkup,
    bindOverviewColumnControls,
    overviewGridTemplate,
    overviewTableMinimumWidth,
    bindOverviewColumnResizing,
  } = App;

  const memoryOverviewControls = {
    sortKey: "memory",
    sortDirection: "asc",
  };
  const memoryOverviewColumns = [
    {
      key: "capacity",
      label: "Capacity",
      colour: "var(--activity)",
      format: formatBytes,
      defaultWidth: 130,
      minWidth: 100,
    },
    {
      key: "allocated",
      label: "Allocated",
      colour: "var(--activity)",
      format: formatBytes,
      defaultWidth: 130,
      minWidth: 100,
    },
    {
      key: "allocated-ratio",
      label: "Allocated %",
      colour: "var(--activity)",
      format: (value) => `${Number(value).toFixed(3)}%`,
      defaultWidth: 120,
      minWidth: 100,
    },
    {
      key: "read",
      label: "Read",
      colour: "var(--read)",
      format: formatBytes,
      defaultWidth: 130,
      minWidth: 100,
    },
    {
      key: "write",
      label: "Written",
      colour: "var(--write)",
      format: formatBytes,
      defaultWidth: 130,
      minWidth: 100,
    },
    {
      key: "tensors",
      label: "Tensors",
      colour: "var(--activity)",
      format: (value) => fmt.format(value),
      defaultWidth: 100,
      minWidth: 85,
    },
    {
      key: "kind",
      label: "Kind",
      defaultWidth: 100,
      minWidth: 80,
    },
  ];
  const memoryOverviewColumnConfiguration = createOverviewColumnConfiguration(
    memoryOverviewColumns,
    memoryOverviewColumns.map((column) => column.key),
  );

  function setMemoryOverviewSort(key, direction) {
    if (
      key !== "memory" &&
      !memoryOverviewColumns.some((column) => column.key === key)
    ) {
      return;
    }
    memoryOverviewControls.sortKey = key;
    memoryOverviewControls.sortDirection =
      direction === "desc" ? "desc" : "asc";
  }

  function filteredMemories() {
    const visibleTensorIds = new Set(
      filteredTensors().map((tensor) => tensor.id),
    );
    return (data.memory?.platform_memories || [])
      .filter((memory) => filterMatches(memoryFilterValue(), memory.name))
      .map((memory) => {
        const tensors = (memory.tensors || []).filter((id) =>
          visibleTensorIds.has(id),
        );
        let allocated = 0n;
        let read = 0n;
        let write = 0n;
        for (const id of tensors) {
          const tensor = tensorsById.get(id);
          if (!tensor) {
            continue;
          }
          const tensorBytes = bigIntMax(toBigInt(tensor.num_bytes), 1n);
          const overlap = overlapBytes(
            tensor.addr,
            tensorBytes,
            memory.base_addr,
            memory.capacity_bytes,
          );
          const traffic = tensorTraffic(tensor);
          allocated += overlap;
          read += scaleTensorToMemory(tensor, traffic.readBytes, memory.name);
          write += scaleTensorToMemory(
            tensor,
            traffic.writtenBytes,
            memory.name,
          );
        }
        return {
          ...memory,
          tensors,
          tensor_count: tensors.length,
          allocated_bytes: allocated,
          read_bytes: read,
          write_bytes: write,
        };
      });
  }

  function memoryTotals(memories) {
    return memories.reduce(
      (totals, memory) => {
        totals.capacity += toBigInt(memory.capacity_bytes);
        totals.allocated += toBigInt(memory.allocated_bytes);
        totals.read += toBigInt(memory.read_bytes);
        totals.write += toBigInt(memory.write_bytes);
        return totals;
      },
      { capacity: 0n, allocated: 0n, read: 0n, write: 0n },
    );
  }

  function memoryAllocationMarkup(allocated, capacity, caption) {
    return metricStripMarkup({
      label: "Memory allocation",
      total: allocated,
      formatter: formatBytes,
      maximum: capacity,
      entries: [
        {
          label: "Allocated",
          value: allocated,
          formatter: formatBytes,
          colour: "var(--activity)",
        },
      ],
      caption,
    });
  }

  function memoryMetrics() {
    const key = cacheKey(
      layerFilterValue(),
      peFilterValue(),
      memoryFilterValue(),
      tensorFilterValue(),
    );
    if (!memoryMetricsCache.has(key)) {
      const memories = filteredMemories();
      memoryMetricsCache.set(key, {
        key,
        memories,
        totals: memoryTotals(memories),
      });
    }
    return memoryMetricsCache.get(key);
  }

  function emptyMemoryMessage() {
    return (data.memory?.platform_memories || []).length
      ? "No memories match the current filters."
      : "Provide a platform for memory details.";
  }

  function selectedMemory(memories) {
    if (!memories.length) {
      state.selectedMemoryName = null;
      return null;
    }
    let memory = memories.find(
      (candidate) => candidate.name === state.selectedMemoryName,
    );
    if (!memory) {
      memory = memories[0];
      state.selectedMemoryName = memory.name;
    }
    return memory;
  }

  function renderMemorySummary() {
    const panel = memorySummary.closest("[data-view]");
    if (panel?.hidden) {
      return;
    }
    const { key, memories, totals } = memoryMetrics();
    if (
      state.renderedMemorySummaryKey === key &&
      memorySummary.childElementCount
    ) {
      return;
    }
    state.renderedMemorySummaryKey = key;
    memorySummary.innerHTML = "";

    if (!memories.length) {
      memorySummary.innerHTML = `<p class="memory-empty">${emptyMemoryMessage()}</p>`;
      return;
    }

    const totalAllocatedPercent = ratioPercent(
      totals.allocated,
      bigIntMax(totals.capacity, 1n),
    );
    memorySummary.innerHTML = `
      <div class="summary-metrics">
        <div><span>Memories</span><strong>${fmt.format(memories.length)}</strong></div>
        <div><span>Capacity</span><strong>${formatBytes(totals.capacity)}</strong></div>
      </div>
      ${memoryAllocationMarkup(
        totals.allocated,
        totals.capacity,
        `${totalAllocatedPercent.toFixed(3)}% of total capacity`,
      )}
      ${trafficMarkup({ read: totals.read, write: totals.write })}
    `;
  }

  function memoryOverviewMetric(memory) {
    const capacity = toBigInt(memory.capacity_bytes);
    const allocated = toBigInt(memory.allocated_bytes);
    return {
      memory,
      capacity,
      allocated,
      "allocated-ratio": ratioPercent(allocated, bigIntMax(capacity, 1n)),
      read: toBigInt(memory.read_bytes),
      write: toBigInt(memory.write_bytes),
      tensors: Number(memory.tensor_count || 0),
      kind: memory.kind,
    };
  }

  function compareMemoryOverviewMetric(left, right, key) {
    const leftValue = key === "memory" ? left.memory.name : left[key];
    const rightValue = key === "memory" ? right.memory.name : right[key];
    if (typeof leftValue === "bigint" || typeof rightValue === "bigint") {
      return bigIntCompare(leftValue, rightValue);
    }
    if (typeof leftValue === "number" && typeof rightValue === "number") {
      return leftValue - rightValue;
    }
    return String(leftValue).localeCompare(String(rightValue), undefined, {
      numeric: true,
    });
  }

  function memoryOverviewColumnStatistics(metrics, columns) {
    return new Map(
      columns
        .filter((column) => column.format)
        .map((column) => {
          const values = metrics.map((metric) => metric[column.key]);
          const hasBigInt = values.some((value) => typeof value === "bigint");
          const maximum = hasBigInt
            ? values.reduce(
                (current, value) => bigIntMax(current, toBigInt(value)),
                1n,
              )
            : Math.max(...values.map(Number), 1);
          const average = hasBigInt
            ? integerAverage(
                values.reduce((total, value) => total + toBigInt(value), 0n),
                values.length,
              )
            : values.reduce((total, value) => total + Number(value), 0) /
              Math.max(values.length, 1);
          return [column.key, { maximum, average }];
        }),
    );
  }

  function memoryOverviewMetricMarkup(metric, column, statistics) {
    if (!column.format) {
      return `<span class="memory-overview-text">${escapeHtml(metric[column.key])}</span>`;
    }
    const value = metric[column.key];
    const formatted = column.format(value);
    const width = Math.min(ratioPercent(value, statistics.maximum), 100);
    const average = Math.min(
      ratioPercent(statistics.average, statistics.maximum),
      100,
    );
    return `
      <span class="memory-overview-metric" title="${escapeHtml(`${column.label}: ${formatted}; average ${column.format(statistics.average)}`)}">
        <strong>${formatted}</strong>
        <span class="memory-overview-track" style="--metric-colour: ${column.colour}">
          <i style="width: ${width}%"></i>
          <b style="left: ${average}%" aria-hidden="true"></b>
        </span>
      </span>`;
  }

  function memoryOverviewHeaderMarkup(key, label) {
    const active = memoryOverviewControls.sortKey === key;
    const direction = active ? memoryOverviewControls.sortDirection : "";
    const button = `<button type="button" data-memory-sort="${escapeHtml(key)}" aria-pressed="${active}" title="Sort by ${escapeHtml(label)}"><span>${escapeHtml(label)}</span><i aria-hidden="true">${direction === "asc" ? "↑" : direction === "desc" ? "↓" : ""}</i></button>`;
    if (key === "memory") {
      return button;
    }
    return `<span class="overview-column-header" data-column-key="${escapeHtml(key)}">${button}<span class="overview-column-resize" data-column-resize="${escapeHtml(key)}" role="separator" aria-orientation="vertical" aria-label="Resize ${escapeHtml(label)} column" tabindex="0" title="Drag to resize ${escapeHtml(label)}; double-click to distribute columns evenly"></span></span>`;
  }

  function renderMemoriesOverview() {
    const panel = memoriesOverview.closest("[data-view]");
    if (panel?.hidden) {
      return;
    }
    const { key, memories } = memoryMetrics();
    const overviewKey = JSON.stringify({
      key,
      selected: state.selectedMemoryName || "",
      sortKey: memoryOverviewControls.sortKey,
      sortDirection: memoryOverviewControls.sortDirection,
      columns: memoryOverviewColumnConfiguration.snapshot(),
    });
    if (
      state.renderedMemoriesOverviewKey === overviewKey &&
      memoriesOverview.childElementCount
    ) {
      return;
    }
    state.renderedMemoriesOverviewKey = overviewKey;
    memoriesOverview.innerHTML = "";

    if (!memories.length) {
      memoriesOverview.innerHTML = `<p class="memory-empty">${emptyMemoryMessage()}</p>`;
      return;
    }
    selectedMemory(memories);

    const columns = memoryOverviewColumnConfiguration.visible();
    const metrics = memories.map(memoryOverviewMetric);
    const statistics = memoryOverviewColumnStatistics(metrics, columns);
    const direction = memoryOverviewControls.sortDirection === "desc" ? -1 : 1;
    metrics.sort(
      (left, right) =>
        direction *
          compareMemoryOverviewMetric(
            left,
            right,
            memoryOverviewControls.sortKey,
          ) ||
        left.memory.name.localeCompare(right.memory.name, undefined, {
          numeric: true,
        }),
    );
    const identityWidth = 150;
    App.bindOverviewKeyboard(
      memoriesOverview,
      metrics.map((metric) => metric.memory.name),
      () => state.selectedMemoryName,
      (name) => App.selectMemory(name),
    );
    const tableWidth = overviewTableMinimumWidth(
      memoryOverviewColumnConfiguration,
      identityWidth,
    );
    const gridTemplate = overviewGridTemplate(
      memoryOverviewColumnConfiguration,
      identityWidth,
    );

    memoriesOverview.innerHTML = `
      <div class="memory-overview-status">Displaying ${fmt.format(metrics.length)} memories · Bars use filtered maximum; markers show filtered average</div>
      ${overviewColumnControlsMarkup(memoryOverviewColumnConfiguration)}
      <div class="memories-overview-table overview-table" style="--overview-grid-template: ${gridTemplate}; --overview-table-width: ${tableWidth}px">
        <div class="memory-overview-header">
          ${memoryOverviewHeaderMarkup("memory", "Memory")}
          ${columns.map((column) => memoryOverviewHeaderMarkup(column.key, column.label)).join("")}
        </div>
        <div class="memories-overview-list"></div>
      </div>`;

    const list = memoriesOverview.querySelector(".memories-overview-list");
    const fragment = document.createDocumentFragment();
    for (const metric of metrics) {
      const { memory } = metric;
      const row = document.createElement("button");
      row.type = "button";
      row.className = "memories-overview-row";
      if (memory.name === state.selectedMemoryName) {
        row.classList.add("selected");
      }
      row.setAttribute(
        "aria-pressed",
        memory.name === state.selectedMemoryName ? "true" : "false",
      );
      row.setAttribute(
        "aria-label",
        `${memory.name}: ${columns
          .map((column) =>
            column.format
              ? `${column.label} ${column.format(metric[column.key])}`
              : `${column.label} ${metric[column.key]}`,
          )
          .join(", ")}`,
      );
      row.innerHTML = `
        <span class="memory-overview-name" title="${escapeHtml(memory.name)}">${escapeHtml(memory.name)}</span>
        ${columns
          .map((column) =>
            memoryOverviewMetricMarkup(
              metric,
              column,
              statistics.get(column.key),
            ),
          )
          .join("")}`;
      bindSelectAndFilter(
        row,
        () => {
          App.selectMemory(memory.name);
        },
        filterPickers.memories,
        memory.name,
      );
      fragment.append(row);
    }
    list.append(fragment);

    bindOverviewColumnControls(
      memoriesOverview,
      memoryOverviewColumnConfiguration,
      [],
      () => {
        if (
          memoryOverviewControls.sortKey !== "memory" &&
          !memoryOverviewColumnConfiguration
            .visible()
            .some((column) => column.key === memoryOverviewControls.sortKey)
        ) {
          setMemoryOverviewSort("memory", "asc");
        }
        renderMemoriesOverview();
        App.workspaceChanged?.();
      },
    );
    bindOverviewColumnResizing(
      memoriesOverview,
      memoryOverviewColumnConfiguration,
      identityWidth,
      () => {
        renderMemoriesOverview();
        App.workspaceChanged?.();
      },
    );
    for (const header of memoriesOverview.querySelectorAll(
      "[data-memory-sort]",
    )) {
      header.addEventListener("click", () => {
        const sortKey = header.dataset.memorySort;
        const sortDirection =
          memoryOverviewControls.sortKey === sortKey &&
          memoryOverviewControls.sortDirection === "desc"
            ? "asc"
            : "desc";
        setMemoryOverviewSort(sortKey, sortDirection);
        renderMemoriesOverview();
        App.workspaceChanged?.();
      });
    }
  }

  function renderMemoryDetail() {
    const panel = memoryDetail.closest("[data-view]");
    if (panel?.hidden) {
      return;
    }
    const memoryKey = cacheKey(
      skipMemoryGaps.checked,
      layerFilterValue(),
      peFilterValue(),
      memoryFilterValue(),
      tensorFilterValue(),
      state.selectedMemoryName || null,
      state.selectedTensor?.id || null,
      memoryVisualMode.value,
    );
    if (
      state.renderedMemoryDetailKey === memoryKey &&
      memoryDetail.querySelector(".memory-detail-list")
    ) {
      return;
    }
    state.renderedMemoryDetailKey = memoryKey;
    memoryDetail.innerHTML = "";

    const memories = memoryMetrics().memories;
    if (!memories.length) {
      const message = (data.memory?.platform_memories || []).length
        ? "No memories match the current filters."
        : "Provide a platform for memory details.";
      memoryDetail.innerHTML = `<p class="memory-empty">${message}</p>`;
      return;
    }
    const memory = selectedMemory(memories);
    if (!memory) {
      memoryDetail.innerHTML = `<p class="memory-empty">Provide a platform for memory details.</p>`;
      return;
    }

    const list = document.createElement("div");
    list.className = "memory-detail-list";

    const capacity = bigIntMax(toBigInt(memory.capacity_bytes), 1n);
    const allocated = toBigInt(memory.allocated_bytes);
    const allocatedPercent = Math.min(ratioPercent(allocated, capacity), 100);
    const read = toBigInt(memory.read_bytes);
    const write = toBigInt(memory.write_bytes);
    const totals = memoryTotals(memories);
    const averageRead = integerAverage(totals.read, memories.length);
    const averageWrite = integerAverage(totals.write, memories.length);
    const maxRead = memories.reduce(
      (maximum, candidate) =>
        bigIntMax(maximum, toBigInt(candidate.read_bytes)),
      1n,
    );
    const maxWrite = memories.reduce(
      (maximum, candidate) =>
        bigIntMax(maximum, toBigInt(candidate.write_bytes)),
      1n,
    );
    const section = document.createElement("section");
    section.className = "memory-detail-card";

    const header = document.createElement("div");
    header.className = "memory-detail-header";
    header.innerHTML = `
      <div>
        <h3>${escapeHtml(memory.name)}</h3>
        <span>${escapeHtml(memory.kind)} · ${formatHex(memory.base_addr)} - ${formatHex(addressRange(memory.base_addr, memory.capacity_bytes)[1])}</span>
      </div>
    `;

    const metrics = document.createElement("div");
    metrics.className = "memory-detail-metrics";
    metrics.innerHTML = `
      ${memoryAllocationMarkup(
        allocated,
        capacity,
        `${allocatedPercent.toFixed(3)}% of ${formatBytes(memory.capacity_bytes)} capacity`,
      )}
      ${trafficMarkup({
        read,
        write,
        maximum: bigIntMax(maxRead, maxWrite),
        averageRead,
        averageWrite,
        caption: `Compared with filtered memories; averages Read ${formatBytes(averageRead)}, Written ${formatBytes(averageWrite)}`,
      })}
    `;

    const layout = document.createElement("div");
    layout.className = `memory-detail-layout memory-mode-${memoryVisualMode.value}`;
    const tensors = (memory.tensors || [])
      .map((id) => tensorsById.get(id))
      .filter(Boolean)
      .sort((a, b) => bigIntCompare(a.addr, b.addr) || a.id.localeCompare(b.id))
      .map((tensor) => clipTensorToMemory(tensor, memory))
      .filter(Boolean);

    if (!tensors.length) {
      layout.innerHTML = `<p class="memory-empty">No tensors allocated in this memory.</p>`;
    } else {
      for (const region of buildTensorRegions(
        tensors,
        skipMemoryGaps.checked,
      )) {
        if (region.gapBefore) {
          const gap = document.createElement("div");
          gap.className = "memory-gap-row";
          gap.textContent = `${formatBytes(region.gapBefore)} unused`;
          layout.append(gap);
        }

        const regionHeader = document.createElement("div");
        regionHeader.className = "memory-region-header";
        regionHeader.innerHTML = `
          <span>${formatHex(region.start)} - ${formatHex(region.end)}</span>
          <strong>${formatBytes(region.allocated)} allocated</strong>
        `;
        layout.append(regionHeader);

        for (const tensorLayout of region.tensors) {
          const tensor = tensorLayout.tensor;
          const addr = toBigInt(tensorLayout.addr);
          const bytes = bigIntMax(toBigInt(tensorLayout.num_bytes), 1n);
          const traffic = tensorTraffic(tensor);
          const left =
            (bigIntToNumber(addr - region.start) /
              bigIntToNumber(region.span)) *
            100;
          const width = Math.max(
            (bigIntToNumber(bytes) / bigIntToNumber(region.span)) * 100,
            0.35,
          );

          const row = document.createElement("div");
          row.className = "memory-tensor-row";
          markSelectionElement(row, filterPickers.tensors, tensor.id);
          if (tensor === state.selectedTensor) {
            row.classList.add("selected");
          }

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
          block.type = "button";
          block.className = "memory-tensor-block";
          if (tensor === state.selectedTensor) {
            block.classList.add("selected");
          }
          block.style.left = `${left}%`;
          block.style.width = `${Math.min(width, 100 - left)}%`;
          block.style.setProperty("--read-share", `${traffic.readShare}%`);
          block.style.setProperty("--write-share", `${traffic.writeShare}%`);
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
          size.textContent = formatBytes(bytes);
          size.title = `${formatBytes(bytes)} overlap at ${formatHex(tensorLayout.addr)}`;
          row.append(label, track, size);
          layout.append(row);
        }
      }
    }

    section.append(header, metrics, layout);
    list.append(section);

    memoryDetail.append(list);
  }

  Object.assign(App, {
    memoryOverviewControls,
    memoryOverviewColumnConfiguration,
    setMemoryOverviewSort,
    renderMemorySummary,
    renderMemoriesOverview,
    renderMemoryDetail,
  });
})();
