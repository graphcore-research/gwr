// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    data,
    fmt,
    state,
    tensorMemory,
    skipMemoryGaps,
    selectedTensorPanel,
    memoryLayoutCache,
    filterPickers,
    filteredTensors,
    tensorTraffic,
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
    addressRange,
    rangeUnionBytes,
    clipTensorToMemory,
    formatBytes,
    formatHex,
    escapeHtml,
  } = App;

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
    );
    if (
      state.renderedTensorMemoryKey === memoryKey &&
      tensorMemory.querySelector(".memory-regions")
    ) {
      return;
    }
    state.renderedTensorMemoryKey = memoryKey;

    const regions = getMemoryLayout(skipMemoryGaps.checked, tensors);
    const allocatedBytes = regions.reduce(
      (sum, region) => sum + region.allocated,
      0n,
    );
    tensorMemory.innerHTML = "";

    const container = document.createElement("div");
    container.className = "memory-regions";
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
            state.selectedTensor = tensor;
            App.selectionChanged("tensor");
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
    <span><i class="tensor"></i>tensor traffic</span>
    <span><i class="read"></i>read %</span>
    <span><i class="write"></i>written %</span>
    <span><i class="gap"></i>unused gap${skipMemoryGaps.checked ? " (collapsed between regions)" : ""}</span>
    <strong>${formatBytes(allocatedBytes)} allocated</strong>
  `;

    tensorMemory.append(container, legend);
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
        current.tensors.push(tensor);
      }
    }

    for (const region of regions) {
      region.span = bigIntMax(region.end - region.start, 1n);
      region.allocated = rangeUnionBytes(
        region.tensors.map((tensor) =>
          addressRange(tensor.addr, tensor.num_bytes),
        ),
      );
    }
    return regions;
  }

  function memoryTensorTitle(tensor, traffic = tensorTraffic(tensor)) {
    const consumerPeCount = traffic.reads.length;
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

    const tensorBytes = toBigInt(state.selectedTensor.num_bytes);
    const selectedTraffic = tensorTraffic(state.selectedTensor);
    const writtenBytes = selectedTraffic.writtenBytes;
    const readBytes = selectedTraffic.readBytes;
    const maxBytes = bigIntMax(
      bigIntMax(tensorBytes, writtenBytes),
      bigIntMax(readBytes, 1n),
    );
    const readRatio = tensorBytes
      ? bigIntToNumber(readBytes) / bigIntToNumber(tensorBytes)
      : 0;
    const writeRatio = tensorBytes
      ? bigIntToNumber(writtenBytes) / bigIntToNumber(tensorBytes)
      : 0;
    const bars = [
      ["Tensor size", tensorBytes, 1, "size"],
      ["Read", readBytes, readRatio, "read"],
      ["Written", writtenBytes, writeRatio, "write"],
    ]
      .map(
        ([label, bytes, ratio, mode]) => `
      <div class="tensor-byte-row">
        <span>${label}</span>
        <div class="tensor-byte-track"><div class="tensor-byte-fill ${mode}" style="width: ${ratioPercent(bytes, maxBytes)}%"></div></div>
        <strong>${formatBytes(bytes)}</strong>
        <em>${Number(ratio).toFixed(2)}×</em>
      </div>
    `,
      )
      .join("");

    selectedTensorPanel.innerHTML = `
    <h2>${escapeHtml(state.selectedTensor.id)}</h2>
    <p>${formatHex(state.selectedTensor.addr)} · ${formatBytes(state.selectedTensor.num_bytes)} · ${escapeHtml(state.selectedTensor.dtype)} [${escapeHtml(state.selectedTensor.shape.join(" × "))}]</p>
    <div class="tensor-byte-summary">${bars}</div>
  `;
  }

  Object.assign(App, {
    renderTensorMemory,
    getMemoryLayout,
    buildTensorRegions,
    memoryTensorTitle,
    renderSelectedTensor,
  });
})();
