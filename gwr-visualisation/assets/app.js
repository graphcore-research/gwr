// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    data,
    controls,
    state,
    pesByName,
    peOverviewControls,
    skipMemoryGaps,
    memoryVisualMode,
    relationshipControls,
    memoryLayoutCache,
    memoryMetricsCache,
    filterContextCache,
    relationshipModelCache,
    allLayerNames,
    allPeNames,
    allMemoryNames,
    allTensorIds,
    filterState,
    layerFilterValue,
    peFilterValue,
    memoryFilterValue,
    filterValues,
    filteredTensors,
    updateFilterSummaries,
    initializeFilterControls,
    initializePeOverviewControls,
    setPeOverviewMode,
    updateRelationshipMeasureOptions,
    initializeWorkspace,
    renderGlobalStats,
    renderTimetableSummary,
    renderLayerSummary,
    renderLayerDetail,
    renderRelationships,
    renderComputeSummary,
    renderPeOverview,
    renderSelected,
    renderTensorOverview,
    renderTensorAccesses,
    renderTensorMemory,
    renderMemorySummary,
    renderMemoriesOverview,
    renderMemoryDetail,
    renderSelectedTensor,
    renderTimetableGraph,
    renderSelectedNode,
    syncSelectedNode,
    resetTensorOverviewPage,
    resetTensorAccessPage,
    revealLayerOverviewSelection,
    revealTensorOverviewSelection,
    revealTensorMemorySelection,
  } = App;

  const panelRenderers = new Map([
    ["timetable-summary", renderTimetableSummary],
    ["layer-summary", renderLayerSummary],
    ["layer-details", renderLayerDetail],
    ["relationships", renderRelationships],
    ["compute-summary", renderComputeSummary],
    ["pe-grid", renderPeOverview],
    ["selected-pe", renderSelected],
    ["tensor-overview", renderTensorOverview],
    ["tensor-accesses", renderTensorAccesses],
    ["tensor-memory", renderTensorMemory],
    ["memory-summary", renderMemorySummary],
    ["memories-overview", renderMemoriesOverview],
    ["memory-details", renderMemoryDetail],
    ["selected-tensor", renderSelectedTensor],
    ["timetable-graph", renderTimetableGraph],
    ["selected-node", renderSelectedNode],
  ]);

  const filterBindings = [
    ["layer", "layers", allLayerNames],
    ["pe", "pes", allPeNames],
    ["memory", "memories", allMemoryNames],
    ["tensor", "tensors", allTensorIds],
  ];
  const allPanelNames = [...panelRenderers.keys()];
  const dirtyPanels = new Set(allPanelNames);
  const selectionDependencies = {
    layer: ["layer-details", "relationships"],
    pe: ["selected-pe", "relationships", "tensor-accesses"],
    memory: ["memory-details", "tensor-memory", "relationships"],
    tensor: ["selected-tensor", "tensor-accesses", "relationships", "pe-grid"],
    node: ["timetable-graph", "selected-node"],
  };
  let globalStatsDirty = true;
  let renderFrame = null;

  function renderWarnings() {
    const warnings = document.getElementById("warnings");
    warnings.replaceChildren(
      ...(data.warnings || []).map((warning) => {
        const message = document.createElement("p");
        message.textContent = warning;
        return message;
      }),
    );
  }

  function markPanelsDirty(names) {
    for (const name of names) {
      dirtyPanels.add(name);
    }
  }

  function scheduleRender(names, includeGlobalStats = false) {
    markPanelsDirty(names);
    globalStatsDirty ||= includeGlobalStats;
    if (renderFrame === null) {
      renderFrame = window.requestAnimationFrame(renderDirtyPanels);
    }
  }

  function invalidateFilteredViews() {
    state.renderedTensorMemoryKey = null;
    state.renderedMemorySummaryKey = null;
    state.renderedMemoriesOverviewKey = null;
    state.renderedMemoryDetailKey = null;
  }

  function syncFilteredSelections() {
    const selectedPes = filterValues(peFilterValue());
    if (selectedPes === null && !state.selectedPe) {
      state.selectedPe =
        data.pes.find((pe) => pe.total_nodes > 0) || data.pes[0] || null;
    } else if (
      selectedPes !== null &&
      !selectedPes.has(state.selectedPe?.name)
    ) {
      state.selectedPe = pesByName.get([...selectedPes][0]) || null;
    }
    const selectedLayers = filterValues(layerFilterValue());
    if (selectedLayers === null && !state.selectedLayerName) {
      state.selectedLayerName = data.layers?.[0]?.name || null;
    } else if (
      selectedLayers !== null &&
      !selectedLayers.has(state.selectedLayerName)
    ) {
      state.selectedLayerName = [...selectedLayers][0] || null;
    }
    const selectedMemories = filterValues(memoryFilterValue());
    if (selectedMemories === null && !state.selectedMemoryName) {
      state.selectedMemoryName = allMemoryNames[0] || null;
    } else if (
      selectedMemories !== null &&
      !selectedMemories.has(state.selectedMemoryName)
    ) {
      state.selectedMemoryName = [...selectedMemories][0] || null;
    }
    const tensors = filteredTensors();
    if (
      !state.selectedTensor ||
      !tensors.some((tensor) => tensor.id === state.selectedTensor.id)
    ) {
      state.selectedTensor = tensors[0] || null;
    }
    const visible = App.workspaceVisibleViews();
    if (["timetable-graph", "selected-node"].some((name) => visible.has(name)))
      syncSelectedNode();
    syncActiveEntity();
  }

  function renderDirtyPanels() {
    if (renderFrame !== null) {
      window.cancelAnimationFrame(renderFrame);
      renderFrame = null;
    }
    syncFilteredSelections();
    if (globalStatsDirty) {
      renderGlobalStats();
      globalStatsDirty = false;
    }
    App.refreshWorkspaceInspector();
    const visible = App.workspaceVisibleViews();
    for (const name of [...dirtyPanels]) {
      if (!visible.has(name)) {
        continue;
      }
      App.viewRegistry.get(name)?.render?.();
      dirtyPanels.delete(name);
    }
  }

  function selectedEntityId(kind) {
    const selected = {
      layer: state.selectedLayerName,
      pe: state.selectedPe?.name,
      memory: state.selectedMemoryName,
      tensor: state.selectedTensor?.id,
    };
    return selected[kind] || null;
  }

  function updateSelectionOutlines(kind) {
    const selectedId = selectedEntityId(kind);
    for (const element of document.querySelectorAll(
      `[data-selection-kind="${kind}"]`,
    )) {
      const selected = element.dataset.selectionId === selectedId;
      element.classList.toggle("selected", selected);
      if (element.hasAttribute("aria-pressed")) {
        element.setAttribute("aria-pressed", selected ? "true" : "false");
      }
    }
  }

  function selectionChanged(kind) {
    updateSelectionOutlines(kind);
    scheduleRender(selectionDependencies[kind] || []);
  }

  function setActiveEntity(kind, id) {
    state.activeEntity = id ? { kind, id } : null;
    markPanelsDirty([
      "selected-tensor",
      "selected-node",
      "layer-details",
      "selected-pe",
      "memory-details",
    ]);
  }

  function selectPe(pe) {
    if (!pe) return;
    state.selectedPe = pe;
    setActiveEntity("pe", pe.name);
    selectionChanged("pe");
  }

  function selectMemory(name) {
    if (!name) return;
    state.selectedMemoryName = name;
    setActiveEntity("memory", name);
    selectionChanged("memory");
  }

  function syncActiveEntity() {
    if (!state.activeEntity) return;
    const { kind, id } = state.activeEntity;
    const eligible = {
      tensor: state.selectedTensor?.id,
      pe: state.selectedPe?.name,
      layer: state.selectedLayerName,
      memory: state.selectedMemoryName,
    };
    if (["compute", "group"].includes(kind)) {
      const candidates =
        kind === "compute"
          ? data.compute_nodes || []
          : data.graph?.compute_groups || [];
      const model = App.graphSelectionModel();
      const candidateId = (value, index) =>
        kind === "compute" ? value.id : String(index);
      const current = candidates.find(
        (value, index) =>
          candidateId(value, index) === id &&
          App.graphSelectionExists({ kind, id }, model),
      );
      const index = current
        ? candidates.indexOf(current)
        : candidates.findIndex((value, index) =>
            App.graphSelectionExists(
              { kind, id: candidateId(value, index) },
              model,
            ),
          );
      eligible[kind] = index < 0 ? null : candidateId(candidates[index], index);
    }
    const next = eligible[kind];
    if (!next) {
      state.activeEntity = { kind, id: null };
      return;
    }
    if (next !== id) setActiveEntity(kind, next);
    if (["tensor", "compute", "group", "layer"].includes(kind))
      state.selectedNode = { kind, id: next };
  }

  function selectTensor(tensor) {
    if (!tensor) {
      return;
    }
    state.selectedTensor = tensor;
    setActiveEntity("tensor", tensor.id);
    state.selectedNode = { kind: "tensor", id: tensor.id };
    App.focusTimetableGraphSelection?.();
    revealTensorOverviewSelection(tensor.id);
    revealTensorMemorySelection(tensor.id);
    markPanelsDirty(["tensor-overview", "tensor-memory"]);
    selectionChanged("tensor");
    selectionChanged("node");
  }

  function selectGraphTensor(tensor) {
    selectTensor(tensor);
  }

  function selectCompute(node) {
    if (!node) {
      return;
    }
    setActiveEntity("compute", node.id);
    state.selectedNode = { kind: "compute", id: node.id };
    state.selectedPe = pesByName.get(node.pe) || state.selectedPe;
    state.selectedLayerName = node.layer || state.selectedLayerName;
    App.focusTimetableGraphSelection?.();
    selectionChanged("node");
    selectionChanged("pe");
    selectionChanged("layer");
  }

  function selectComputeGroup(group) {
    if (!group) {
      return;
    }
    setActiveEntity("group", String(group.index));
    state.selectedNode = { kind: "group", id: String(group.index) };
    App.focusTimetableGraphSelection?.();
    selectionChanged("node");
  }

  function selectLayer(layer, { revealOverview = false } = {}) {
    if (!layer) {
      return;
    }
    state.selectedLayerName = layer;
    setActiveEntity("layer", layer);
    state.selectedNode = { kind: "layer", id: layer };
    App.focusTimetableGraphSelection?.();
    if (revealOverview && revealLayerOverviewSelection(layer)) {
      markPanelsDirty(["layer-summary"]);
    }
    selectionChanged("layer");
    selectionChanged("node");
  }

  function selectGraphLayer(layer) {
    selectLayer(layer, { revealOverview: true });
  }

  peOverviewControls.measure.addEventListener("change", () => {
    App.setPeOverviewMeasure(peOverviewControls.measure.value);
    scheduleRender(["pe-grid"]);
    App.workspaceChanged?.();
  });
  document.addEventListener("click", (event) => {
    const action = event.target.closest("[data-pe-overview-measure]");
    if (
      !action ||
      !App.setPeOverviewMeasure(action.dataset.peOverviewMeasure)
    ) {
      return;
    }
    scheduleRender(["pe-grid"]);
    App.workspaceChanged?.();
  });
  for (const button of peOverviewControls.modes) {
    button.addEventListener("click", () => {
      setPeOverviewMode(button.dataset.peOverviewMode);
      scheduleRender(["pe-grid"]);
      App.workspaceChanged?.();
    });
  }
  for (const button of peOverviewControls.gridEncodings) {
    button.addEventListener("click", () => {
      App.setPeGridEncoding(button.dataset.peGridEncoding);
      scheduleRender(["pe-grid"]);
      App.workspaceChanged?.();
    });
  }
  peOverviewControls.scale.addEventListener("change", () => {
    App.setPeScaleMode(peOverviewControls.scale.value);
    scheduleRender(["pe-grid"]);
    App.workspaceChanged?.();
  });
  peOverviewControls.fixedMaximum.addEventListener("input", () => {
    scheduleRender(["pe-grid"]);
    App.workspaceChanged?.();
  });

  function filtersChanged() {
    updateFilterSummaries();
    memoryLayoutCache.clear();
    App.tensorOverviewMetricsCache.clear();
    resetTensorOverviewPage();
    resetTensorAccessPage();
    memoryMetricsCache.clear();
    filterContextCache.clear();
    relationshipModelCache.clear();
    App.invalidateTimetableGraph?.();
    invalidateFilteredViews();
    scheduleRender(allPanelNames, true);
  }

  function handleFilterOptionChange(event, selected) {
    if (
      event.target instanceof HTMLInputElement &&
      event.target.type === "checkbox"
    ) {
      if (event.target.checked) {
        selected.add(event.target.value);
      } else {
        selected.delete(event.target.value);
      }
      filtersChanged();
    }
  }

  function setFilterOptions(container, selected, values, checked) {
    selected.clear();
    if (checked) {
      for (const value of values) {
        selected.add(value);
      }
    }
    for (const input of container.querySelectorAll('input[type="checkbox"]')) {
      input.checked = checked;
    }
    filtersChanged();
  }

  function bindFilterControls() {
    for (const [name, stateKey, values] of filterBindings) {
      const selected = filterState[stateKey];
      controls[name].addEventListener("change", (event) => {
        handleFilterOptionChange(event, selected);
      });
      for (const [action, checked] of [
        ["all", true],
        ["none", false],
      ]) {
        document
          .getElementById(`${name}-filter-${action}`)
          .addEventListener("click", () => {
            setFilterOptions(controls[name], selected, values, checked);
          });
      }
    }
  }

  bindFilterControls();

  relationshipControls.mode.addEventListener("change", () => {
    updateRelationshipMeasureOptions();
    scheduleRender(["relationships"]);
    App.workspaceChanged();
  });
  relationshipControls.measure.addEventListener("change", () => {
    scheduleRender(["relationships"]);
    App.workspaceChanged();
  });
  relationshipControls.limit.addEventListener("change", () => {
    scheduleRender(["relationships"]);
    App.workspaceChanged?.();
  });
  relationshipControls.strength.addEventListener("input", () => {
    scheduleRender(["relationships"]);
    App.workspaceChanged();
  });
  skipMemoryGaps.addEventListener("change", () => {
    scheduleRender(["tensor-memory", "memory-details"]);
    App.workspaceChanged?.();
  });
  memoryVisualMode.addEventListener("change", () => {
    state.renderedTensorMemoryKey = null;
    scheduleRender(["tensor-memory", "memory-details"]);
    App.workspaceChanged?.();
  });

  // These callbacks are used by modules whose handlers are initialized below.
  Object.assign(App, {
    selectionChanged,
    selectTensor,
    selectPe,
    selectMemory,
    selectGraphTensor,
    selectCompute,
    selectComputeGroup,
    selectGraphLayer,
    selectLayer,
    filtersChanged,
    markViewsDirty: markPanelsDirty,
    markAllViewsDirty: () => markPanelsDirty(allPanelNames),
    scheduleVisibleViews: () => scheduleRender([]),
  });
  state.activeEntity = state.selectedTensor
    ? { kind: "tensor", id: state.selectedTensor.id }
    : null;
  initializeFilterControls();
  initializePeOverviewControls();
  updateRelationshipMeasureOptions();
  initializeWorkspace(panelRenderers, selectionDependencies);
  renderWarnings();
})();
