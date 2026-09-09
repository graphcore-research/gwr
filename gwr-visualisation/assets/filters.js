// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    data,
    fmt,
    controls,
    filterContextCache,
    machineOpKeys,
    emptyMachineOps,
    toBigInt,
    bigIntMax,
    bigIntToNumber,
    addressRange,
    intersectRanges,
    rangeUnionBytes,
    trafficForTransfers,
  } = App;

  const SELECTION_CLICK_DELAY_MS = 220;
  const MAX_FILTER_OPTIONS = 500;
  const ALL_FILTER = Symbol("all-filter");

  const allLayerNames = (data.layers || []).map((layer) => layer.name);
  const allPeNames = data.pes.map((pe) => pe.name);
  const allMemoryNames = (data.memory?.platform_memories || []).map(
    (memory) => memory.name,
  );
  const allTensorIds = (data.tensors || []).map((tensor) => tensor.id);
  const filterState = {
    layers: new Set(allLayerNames),
    pes: new Set(allPeNames),
    memories: new Set(allMemoryNames),
    tensors: new Set(allTensorIds),
  };
  const filterPickers = {
    layers: {
      kind: "layer",
      container: controls.layer,
      input: document.getElementById("layer-filter-pattern"),
      status: document.getElementById("layer-filter-pattern-status"),
      selectMatches: document.getElementById("layer-filter-select-matches"),
      clearPattern: document.getElementById("layer-filter-clear-pattern"),
      values: allLayerNames,
      selected: filterState.layers,
    },
    pes: {
      kind: "pe",
      container: controls.pe,
      input: document.getElementById("pe-filter-pattern"),
      status: document.getElementById("pe-filter-pattern-status"),
      selectMatches: document.getElementById("pe-filter-select-matches"),
      clearPattern: document.getElementById("pe-filter-clear-pattern"),
      values: allPeNames,
      selected: filterState.pes,
    },
    memories: {
      kind: "memory",
      container: controls.memory,
      input: document.getElementById("memory-filter-pattern"),
      status: document.getElementById("memory-filter-pattern-status"),
      selectMatches: document.getElementById("memory-filter-select-matches"),
      clearPattern: document.getElementById("memory-filter-clear-pattern"),
      values: allMemoryNames,
      selected: filterState.memories,
    },
    tensors: {
      kind: "tensor",
      container: controls.tensor,
      input: document.getElementById("tensor-filter-pattern"),
      status: document.getElementById("tensor-filter-pattern-status"),
      selectMatches: document.getElementById("tensor-filter-select-matches"),
      clearPattern: document.getElementById("tensor-filter-clear-pattern"),
      values: allTensorIds,
      selected: filterState.tensors,
    },
  };

  function filterValue(selected, allValues) {
    if (selected.size === allValues.length) {
      return ALL_FILTER;
    }
    return allValues.filter((value) => selected.has(value));
  }

  function layerFilterValue() {
    return filterValue(filterState.layers, allLayerNames);
  }

  function peFilterValue() {
    return filterValue(filterState.pes, allPeNames);
  }

  function memoryFilterValue() {
    return filterValue(filterState.memories, allMemoryNames);
  }

  function tensorFilterValue() {
    return filterValue(filterState.tensors, allTensorIds);
  }

  function filterValues(value) {
    if (isAllFilter(value)) {
      return null;
    }
    return new Set(Array.isArray(value) ? value : [value]);
  }

  function isAllFilter(value) {
    return value === ALL_FILTER;
  }

  function filterMatches(value, candidate) {
    const values = filterValues(value);
    return values === null || values.has(candidate);
  }

  function cacheKey(...parts) {
    return JSON.stringify(
      parts.map((value) => (isAllFilter(value) ? { all: true } : { value })),
    );
  }

  function selectedFilterLabel(selected, allValues, noun) {
    if (selected.size === allValues.length) {
      return `All ${fmt.format(allValues.length)}`;
    }
    if (!selected.size) {
      return "None";
    }
    if (selected.size === 1) {
      return [...selected][0];
    }
    return `${fmt.format(selected.size)} ${noun}`;
  }

  function updateFilterSummaries() {
    controls.layerSummary.textContent = selectedFilterLabel(
      filterState.layers,
      allLayerNames,
      "layers",
    );
    controls.peSummary.textContent = selectedFilterLabel(
      filterState.pes,
      allPeNames,
      "PEs",
    );
    controls.memorySummary.textContent = selectedFilterLabel(
      filterState.memories,
      allMemoryNames,
      "memories",
    );
    controls.tensorSummary.textContent = selectedFilterLabel(
      filterState.tensors,
      allTensorIds,
      "tensors",
    );
  }

  function renderFilterOptions(picker, values) {
    picker.container.replaceChildren();
    for (const value of values.slice(0, MAX_FILTER_OPTIONS)) {
      const label = document.createElement("label");
      label.className = "filter-option";
      const input = document.createElement("input");
      input.type = "checkbox";
      input.value = value;
      input.checked = picker.selected.has(value);
      const text = document.createElement("span");
      text.textContent = value;
      label.append(input, text);
      picker.container.append(label);
    }
  }

  function ensureFilterOptions(picker) {
    if (picker.optionsInitialized) {
      return;
    }
    picker.optionsInitialized = true;
    matchingFilterValues(picker);
  }

  function matchingFilterValues(picker) {
    const source = picker.input.value;
    let expression;
    try {
      expression = source ? new RegExp(source, "i") : null;
      picker.input.removeAttribute("aria-invalid");
    } catch {
      picker.input.setAttribute("aria-invalid", "true");
      picker.status.textContent = "Invalid regular expression";
      return null;
    }
    const matches = expression
      ? picker.values.filter((value) => expression.test(value))
      : picker.values;
    if (picker.optionsInitialized) {
      renderFilterOptions(picker, matches);
    }
    const shown = Math.min(matches.length, MAX_FILTER_OPTIONS);
    picker.status.textContent =
      matches.length > shown
        ? `${fmt.format(matches.length)} matches; showing ${fmt.format(shown)}`
        : `${fmt.format(shown)} shown`;
    return matches;
  }

  function selectMatchingFilterValues(picker) {
    const matches = matchingFilterValues(picker);
    if (matches === null) {
      return;
    }
    picker.selected.clear();
    for (const value of matches) {
      picker.selected.add(value);
    }
    for (const input of picker.container.querySelectorAll(
      'input[type="checkbox"]',
    )) {
      input.checked = picker.selected.has(input.value);
    }
    App.filtersChanged();
  }

  function clearFilterPattern(picker) {
    picker.input.value = "";
    matchingFilterValues(picker);
    picker.input.focus();
  }

  function selectOnlyFilterValue(picker, value) {
    if (!picker.values.includes(value)) {
      return;
    }
    picker.selected.clear();
    picker.selected.add(value);
    for (const input of picker.container.querySelectorAll(
      'input[type="checkbox"]',
    )) {
      input.checked = input.value === value;
    }
    App.filtersChanged();
  }

  function bindSelectAndFilter(element, select, picker, value) {
    markSelectionElement(element, picker, value);
    let selectTimer = null;
    element.addEventListener("click", () => {
      clearTimeout(selectTimer);
      selectTimer = setTimeout(select, SELECTION_CLICK_DELAY_MS);
    });
    element.addEventListener("dblclick", (event) => {
      event.preventDefault();
      clearTimeout(selectTimer);
      selectOnlyFilterValue(picker, value);
    });
  }

  function markSelectionElement(element, picker, value) {
    element.dataset.selectionKind = picker.kind;
    element.dataset.selectionId = value;
  }

  function initializeFilterControls() {
    for (const picker of Object.values(filterPickers)) {
      const details = picker.container.closest("details");
      details.addEventListener("toggle", () => {
        if (details.open) {
          ensureFilterOptions(picker);
        }
      });
      picker.input.addEventListener("input", () =>
        matchingFilterValues(picker),
      );
      picker.input.addEventListener("keydown", (event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          selectMatchingFilterValues(picker);
        }
      });
      picker.selectMatches.addEventListener("click", () =>
        selectMatchingFilterValues(picker),
      );
      picker.clearPattern.addEventListener("click", () =>
        clearFilterPattern(picker),
      );
      matchingFilterValues(picker);
    }
    updateFilterSummaries();
  }

  function connectionTrafficFor(
    tensor,
    connection,
    layerSelection = layerFilterValue(),
    memorySelection = memoryFilterValue(),
  ) {
    return trafficForTransfers(
      connection.transfers,
      tensor.addr,
      filterValues(layerSelection),
      selectedMemoryRanges(memorySelection),
    );
  }

  function selectedMemoryRanges(memorySelection = memoryFilterValue()) {
    if (isAllFilter(memorySelection)) {
      return null;
    }
    const selectedMemories = filterValues(memorySelection);
    return (data.memory?.platform_memories || [])
      .filter((memory) => selectedMemories.has(memory.name))
      .map((memory) => addressRange(memory.base_addr, memory.capacity_bytes));
  }

  function visibleConnections(
    tensor,
    connections,
    layerSelection = layerFilterValue(),
    peSelection = peFilterValue(),
    memorySelection = memoryFilterValue(),
  ) {
    return (connections || [])
      .filter((connection) => filterMatches(peSelection, connection.pe))
      .map((connection) => ({
        ...connection,
        ...connectionTrafficFor(
          tensor,
          connection,
          layerSelection,
          memorySelection,
        ),
      }))
      .filter(
        (connection) => connection.bytes > 0n || connection.edgeCount > 0,
      );
  }

  function tensorTrafficFor(
    tensor,
    layerSelection = layerFilterValue(),
    peSelection = peFilterValue(),
    memorySelection = memoryFilterValue(),
  ) {
    const tensorBytes = bigIntMax(toBigInt(tensor.num_bytes), 1n);
    const writes = visibleConnections(
      tensor,
      tensor.writes_by_pe,
      layerSelection,
      peSelection,
      memorySelection,
    );
    const reads = visibleConnections(
      tensor,
      tensor.reads_by_pe,
      layerSelection,
      peSelection,
      memorySelection,
    );
    const writtenBytes = writes.reduce(
      (sum, connection) => sum + connection.bytes,
      0n,
    );
    const readBytes = reads.reduce(
      (sum, connection) => sum + connection.bytes,
      0n,
    );
    const writeRatio =
      bigIntToNumber(writtenBytes) / bigIntToNumber(tensorBytes);
    const readRatio = bigIntToNumber(readBytes) / bigIntToNumber(tensorBytes);
    return {
      writes,
      reads,
      writtenBytes,
      readBytes,
      edgeCount: [...writes, ...reads].reduce(
        (sum, connection) => sum + connection.edgeCount,
        0,
      ),
      writeRatio,
      readRatio,
      writeShare: Math.min(writeRatio * 100, 100),
      readShare: Math.min(readRatio * 100, 100),
    };
  }

  function tensorTraffic(tensor) {
    return tensorTrafficFor(tensor);
  }

  function tensorMemoryOverlapBytes(
    tensor,
    memorySelection = memoryFilterValue(),
  ) {
    const tensorBytes = bigIntMax(toBigInt(tensor.num_bytes), 1n);
    if (isAllFilter(memorySelection)) {
      return tensorBytes;
    }
    const tensorRange = addressRange(tensor.addr, tensorBytes);
    return rangeUnionBytes(
      selectedMemoryRanges(memorySelection).map((memoryRange) =>
        intersectRanges(tensorRange, memoryRange),
      ),
    );
  }

  function tensorMemoryShare(tensor, memorySelection = memoryFilterValue()) {
    const tensorBytes = bigIntMax(toBigInt(tensor.num_bytes), 1n);
    return (
      bigIntToNumber(tensorMemoryOverlapBytes(tensor, memorySelection)) /
      bigIntToNumber(tensorBytes)
    );
  }

  function contextSnapshot(
    layerSelection = layerFilterValue(),
    peSelection = peFilterValue(),
    tensorSelection = tensorFilterValue(),
    memorySelection = memoryFilterValue(),
  ) {
    const key = cacheKey(
      layerSelection,
      peSelection,
      tensorSelection,
      memorySelection,
    );
    if (filterContextCache.has(key)) {
      return filterContextCache.get(key);
    }
    const trafficUnfiltered =
      isAllFilter(layerSelection) && isAllFilter(peSelection);
    const snapshot = (data.tensors || []).reduce(
      (result, tensor) => {
        if (!filterMatches(tensorSelection, tensor.id)) {
          return result;
        }
        const memoryShare = tensorMemoryShare(tensor, memorySelection);
        if (memoryShare === 0) {
          return result;
        }
        const traffic = tensorTrafficFor(
          tensor,
          layerSelection,
          peSelection,
          memorySelection,
        );
        if (
          trafficUnfiltered ||
          traffic.edgeCount > 0 ||
          traffic.readBytes > 0 ||
          traffic.writtenBytes > 0
        ) {
          result.tensors.push(tensor);
          result.readBytes += traffic.readBytes;
          result.writeBytes += traffic.writtenBytes;
          result.dataEdges += traffic.edgeCount;
        }
        return result;
      },
      { tensors: [], readBytes: 0n, writeBytes: 0n, dataEdges: 0 },
    );
    filterContextCache.set(key, snapshot);
    return snapshot;
  }

  function tensorsForContext(
    layerSelection = layerFilterValue(),
    peSelection = peFilterValue(),
    tensorSelection = tensorFilterValue(),
    memorySelection = memoryFilterValue(),
  ) {
    return contextSnapshot(
      layerSelection,
      peSelection,
      tensorSelection,
      memorySelection,
    ).tensors;
  }

  function filteredTensors() {
    return tensorsForContext();
  }

  function filteredLayers(layerSelection = layerFilterValue()) {
    return (data.layers || []).filter((layer) =>
      filterMatches(layerSelection, layer.name),
    );
  }

  function aggregateLayer(layer, peSelection = peFilterValue()) {
    if (isAllFilter(peSelection)) {
      return {
        computeNodes: Number(layer.compute_nodes || 0),
        computeNodesByOp: { ...(layer.by_op || {}) },
        machineOps: { ...(layer.machine_ops || {}) },
        activePeNames: new Set(
          (layer.pes || [])
            .filter((pe) => Number(pe.compute_nodes || 0) > 0)
            .map((pe) => pe.name),
        ),
      };
    }
    const matchingPes = (layer.pes || []).filter((pe) =>
      filterMatches(peSelection, pe.name),
    );
    return matchingPes.reduce(
      (total, pe) => {
        total.computeNodes += Number(pe.compute_nodes || 0);
        for (const [op, count] of Object.entries(pe.by_op || {})) {
          total.computeNodesByOp[op] =
            Number(total.computeNodesByOp[op] || 0) + Number(count || 0);
        }
        for (const key of ["total", ...machineOpKeys]) {
          total.machineOps[key] += toBigInt(pe.machine_ops?.[key]);
        }
        if (Number(pe.compute_nodes || 0) > 0) {
          total.activePeNames.add(pe.name);
        }
        return total;
      },
      {
        computeNodes: 0,
        computeNodesByOp: {},
        machineOps: emptyMachineOps(),
        activePeNames: new Set(),
      },
    );
  }

  function filteredCompute() {
    return filteredLayers().reduce(
      (total, layer) => {
        const aggregate = aggregateLayer(layer);
        total.computeNodes += aggregate.computeNodes;
        for (const [op, count] of Object.entries(aggregate.computeNodesByOp)) {
          total.computeNodesByOp[op] =
            Number(total.computeNodesByOp[op] || 0) + Number(count || 0);
        }
        for (const key of ["total", ...machineOpKeys]) {
          total.machineOps[key] += toBigInt(aggregate.machineOps[key]);
        }
        for (const peName of aggregate.activePeNames) {
          total.activePeNames.add(peName);
        }
        total.activePes = total.activePeNames.size;
        return total;
      },
      {
        computeNodes: 0,
        computeNodesByOp: {},
        machineOps: emptyMachineOps(),
        activePes: 0,
        activePeNames: new Set(),
      },
    );
  }

  function filteredSummary() {
    const layerSelection = layerFilterValue();
    const peSelection = peFilterValue();
    const tensorSelection = tensorFilterValue();
    const memorySelection = memoryFilterValue();
    const compute = filteredCompute();
    const context = contextSnapshot(
      layerSelection,
      peSelection,
      tensorSelection,
      memorySelection,
    );
    const activePes = isAllFilter(layerSelection)
      ? data.pes.filter(
          (pe) =>
            pe.present_in_timetable &&
            Number(pe.total_nodes || 0) > 0 &&
            filterMatches(peSelection, pe.name),
        ).length
      : compute.activePes;
    const unfiltered =
      isAllFilter(layerSelection) &&
      isAllFilter(peSelection) &&
      isAllFilter(tensorSelection) &&
      isAllFilter(memorySelection);
    return {
      ...compute,
      activePes,
      tensors: context.tensors,
      readBytes: unfiltered
        ? toBigInt(data.summary.total_tensor_read_bytes)
        : context.readBytes,
      writeBytes: unfiltered
        ? toBigInt(data.summary.total_tensor_write_bytes)
        : context.writeBytes,
      dataEdges: unfiltered
        ? Number(data.summary.data_edges || 0)
        : context.dataEdges,
    };
  }

  function machineOpsFor(pe, layerSelection = layerFilterValue()) {
    if (isAllFilter(layerSelection)) {
      return pe.machine_ops;
    }
    return [...filterValues(layerSelection)].reduce((total, layerName) => {
      const ops = pe.machine_ops_by_layer?.[layerName] || {};
      for (const key of ["total", ...machineOpKeys]) {
        total[key] += toBigInt(ops[key]);
      }
      return total;
    }, emptyMachineOps());
  }

  function valueFor(pe, layerSelection = layerFilterValue()) {
    if (!pe) {
      return 0;
    }
    const ops = machineOpsFor(pe, layerSelection) || {};
    return toBigInt(ops.total);
  }

  Object.assign(App, {
    allLayerNames,
    allPeNames,
    allMemoryNames,
    allTensorIds,
    filterState,
    filterPickers,
    layerFilterValue,
    peFilterValue,
    memoryFilterValue,
    tensorFilterValue,
    filterValues,
    isAllFilter,
    filterMatches,
    cacheKey,
    updateFilterSummaries,
    selectOnlyFilterValue,
    bindSelectAndFilter,
    markSelectionElement,
    initializeFilterControls,
    tensorTrafficFor,
    tensorTraffic,
    tensorMemoryShare,
    contextSnapshot,
    tensorsForContext,
    filteredTensors,
    filteredLayers,
    aggregateLayer,
    filteredSummary,
    machineOpsFor,
    valueFor,
  });
})();
