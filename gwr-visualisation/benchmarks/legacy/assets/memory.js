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
    filterPickers,
    filteredTensors,
    tensorTrafficFor,
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
    rangeUnionBytes,
    clipTensorToMemory,
    formatBytes,
    formatHex,
    escapeHtml,
    metricBreakdownMarkup,
    comparisonMetricsMarkup,
    buildTensorRegions,
    memoryTensorTitle,
  } = App;

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
        const allocationRanges = [];
        let read = 0n;
        let write = 0n;
        for (const id of tensors) {
          const tensor = tensorsById.get(id);
          if (!tensor) {
            continue;
          }
          const clipped = clipTensorToMemory(tensor, memory);
          if (clipped) {
            allocationRanges.push(
              addressRange(clipped.addr, clipped.num_bytes),
            );
          }
          const traffic = tensorTrafficFor(
            tensor,
            layerFilterValue(),
            peFilterValue(),
            memory.name,
          );
          read += traffic.readBytes;
          write += traffic.writtenBytes;
        }
        return {
          ...memory,
          tensors,
          tensor_count: tensors.length,
          allocated_bytes: rangeUnionBytes(allocationRanges),
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

    const totalAllocatedPercent = totals.capacity
      ? ratioPercent(totals.allocated, totals.capacity)
      : 0;
    memorySummary.innerHTML = metricBreakdownMarkup(
      "Memories",
      memories.length,
      [
        ["Capacity", totals.capacity, formatBytes],
        [
          "Allocated",
          totals.allocated,
          (value) =>
            `${formatBytes(value)} (${totalAllocatedPercent.toFixed(3)}%)`,
        ],
        ["Read", totals.read, formatBytes],
        ["Written", totals.write, formatBytes],
      ],
      true,
    );
  }

  function renderMemoriesOverview() {
    const panel = memoriesOverview.closest("[data-view]");
    if (panel?.hidden) {
      return;
    }
    const { key, memories, totals } = memoryMetrics();
    const overviewKey = `${key}:${state.selectedMemoryName || ""}`;
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

    const maxAllocated = memories.reduce(
      (maximum, memory) => bigIntMax(maximum, toBigInt(memory.allocated_bytes)),
      1n,
    );
    const maxRead = memories.reduce(
      (maximum, memory) => bigIntMax(maximum, toBigInt(memory.read_bytes)),
      1n,
    );
    const maxWrite = memories.reduce(
      (maximum, memory) => bigIntMax(maximum, toBigInt(memory.write_bytes)),
      1n,
    );
    const averageRead = integerAverage(totals.read, memories.length);
    const averageWrite = integerAverage(totals.write, memories.length);
    const list = document.createElement("div");
    list.className = "memories-overview-list";
    for (const memory of memories) {
      const capacity = bigIntMax(toBigInt(memory.capacity_bytes), 1n);
      const allocated = toBigInt(memory.allocated_bytes);
      const read = toBigInt(memory.read_bytes);
      const write = toBigInt(memory.write_bytes);
      const row = document.createElement("button");
      row.type = "button";
      row.className = "memories-overview-row comparison-row";
      if (memory.name === state.selectedMemoryName) {
        row.classList.add("selected");
      }
      row.setAttribute(
        "aria-pressed",
        memory.name === state.selectedMemoryName ? "true" : "false",
      );
      row.setAttribute(
        "aria-label",
        `${memory.name}: ${formatBytes(allocated)} allocated, ${formatBytes(read)} read, ${formatBytes(write)} written`,
      );
      row.innerHTML = `
      <div class="comparison-heading">
        <strong>${escapeHtml(memory.name)}</strong>
        <span>${escapeHtml(memory.kind)} · ${fmt.format(memory.tensor_count || 0)} tensors</span>
      </div>
      ${comparisonMetricsMarkup(
        [
          {
            label: "Allocated",
            value: allocated,
            formatted: `${formatBytes(allocated)} <em>${ratioPercent(allocated, capacity).toFixed(3)}%</em>`,
            mode: "allocated",
            maximum: maxAllocated,
          },
          {
            label: "Read",
            value: read,
            formatted: formatBytes(read),
            mode: "read",
            maximum: maxRead,
            marker: ratioPercent(averageRead, maxRead),
          },
          {
            label: "Written",
            value: write,
            formatted: formatBytes(write),
            mode: "write",
            maximum: maxWrite,
            marker: ratioPercent(averageWrite, maxWrite),
          },
        ],
        "memory-comparison-metrics",
      )}
    `;
      bindSelectAndFilter(
        row,
        () => {
          state.selectedMemoryName = memory.name;
          App.selectionChanged("memory");
        },
        filterPickers.memories,
        memory.name,
      );
      list.append(row);
    }

    memoriesOverview.append(list);
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
      <strong>${formatBytes(allocated)} / ${formatBytes(memory.capacity_bytes)} allocated (${allocatedPercent.toFixed(3)}%)</strong>
    `;

    const meter = document.createElement("div");
    meter.className = "memory-detail-meter";
    meter.innerHTML = `<div style="width: ${allocatedPercent}%"></div>`;

    const traffic = document.createElement("div");
    traffic.className = "memory-detail-traffic";
    traffic.innerHTML = `
      <div class="memory-detail-traffic-row">
        <span>Read</span>
        <div class="memory-detail-traffic-track read"><div style="width: ${ratioPercent(read, maxRead)}%"></div><i style="left: ${ratioPercent(averageRead, maxRead)}%" aria-hidden="true"></i></div>
        <strong>${formatBytes(read)}</strong>
        <em>${ratioPercent(read, maxRead).toFixed(1)}% of maximum · average ${formatBytes(averageRead)}</em>
      </div>
      <div class="memory-detail-traffic-row">
        <span>Written</span>
        <div class="memory-detail-traffic-track write"><div style="width: ${ratioPercent(write, maxWrite)}%"></div><i style="left: ${ratioPercent(averageWrite, maxWrite)}%" aria-hidden="true"></i></div>
        <strong>${formatBytes(write)}</strong>
        <em>${ratioPercent(write, maxWrite).toFixed(1)}% of maximum · average ${formatBytes(averageWrite)}</em>
      </div>
    `;

    const layout = document.createElement("div");
    layout.className = "memory-detail-layout";
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
          const traffic = tensorTrafficFor(
            tensor,
            layerFilterValue(),
            peFilterValue(),
            memory.name,
          );
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
              state.selectedTensor = tensor;
              App.selectionChanged("tensor");
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
          block.title = memoryTensorTitle(tensor, traffic);
          block.setAttribute("aria-label", memoryTensorTitle(tensor, traffic));
          block.innerHTML = `
            <span class="memory-tensor-fill read"></span>
            <span class="memory-tensor-fill write"></span>
          `;
          bindSelectAndFilter(
            block,
            () => {
              state.selectedTensor = tensor;
              App.selectionChanged("tensor");
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

    section.append(header, meter, traffic, layout);
    list.append(section);

    memoryDetail.append(list);
  }

  Object.assign(App, {
    renderMemorySummary,
    renderMemoriesOverview,
    renderMemoryDetail,
  });
})();
