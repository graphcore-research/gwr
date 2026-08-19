// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    data,
    controls,
    viewControls,
    state,
    pesByName,
    peOverviewControls,
    skipMemoryGaps,
    relationshipControls,
    memoryLayoutCache,
    memoryMetricsCache,
    filterContextCache,
    relationshipModelCache,
    viewPresets,
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
    renderTensorMemory,
    renderMemorySummary,
    renderMemoriesOverview,
    renderMemoryDetail,
    renderSelectedTensor,
  } = App;

  const panelRenderers = new Map([
    ["timetable-summary", renderTimetableSummary],
    ["layer-summary", renderLayerSummary],
    ["layer-details", renderLayerDetail],
    ["relationships", renderRelationships],
    ["compute-summary", renderComputeSummary],
    ["pe-grid", renderPeOverview],
    ["selected-pe", renderSelected],
    ["tensor-memory", renderTensorMemory],
    ["memory-summary", renderMemorySummary],
    ["memories-overview", renderMemoriesOverview],
    ["memory-details", renderMemoryDetail],
    ["selected-tensor", renderSelectedTensor],
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
    pe: ["selected-pe", "relationships"],
    memory: ["memory-details", "relationships"],
    tensor: ["selected-tensor", "relationships", "pe-grid"],
  };
  let globalStatsDirty = true;
  let renderFrame = null;

  function visibleViewNames() {
    return new Set(
      viewControls.toggles
        .filter((toggle) => toggle.checked)
        .map((toggle) => toggle.dataset.viewToggle),
    );
  }

  function applyViewConfig(activePreset = null) {
    const visible = visibleViewNames();
    for (const name of ["auto", "one", "two", "three"]) {
      viewControls.views.classList.remove(`layout-${name}`);
    }
    viewControls.views.classList.add(`layout-${viewControls.layout.value}`);
    for (const panel of viewControls.panels) {
      panel.hidden = !visible.has(panel.dataset.view);
    }
    App.reconcileWorkspaceFocus?.(visible);
    for (const button of viewControls.presets) {
      button.setAttribute(
        "aria-pressed",
        button.dataset.preset === activePreset ? "true" : "false",
      );
    }
    renderDirtyPanels();
    App.workspaceChanged?.();
  }

  function setViewPreset(name) {
    if (name === "compute" || name === "memory") {
      relationshipControls.mode.value = name;
      updateRelationshipMeasureOptions();
      peOverviewControls.measure.value =
        name === "compute" ? "compute:machine-ops" : "data:total";
      setPeOverviewMode(name === "compute" ? "grid" : "chart");
    } else if (name === "tensor") {
      peOverviewControls.measure.value = "tensor:read";
      setPeOverviewMode("grid");
    }
    markPanelsDirty(["pe-grid", "relationships"]);
    const preset = viewPresets[name] || viewPresets.layers;
    const currentPanels = [...viewControls.views.children];
    const panelByName = new Map(
      currentPanels.map((panel) => [panel.dataset.view, panel]),
    );
    for (const panelName of preset) {
      const panel = panelByName.get(panelName);
      if (panel) {
        viewControls.views.append(panel);
        panelByName.delete(panelName);
      }
    }
    for (const panel of currentPanels) {
      if (panelByName.has(panel.dataset.view)) {
        viewControls.views.append(panel);
      }
    }

    const visible = new Set(preset);
    for (const toggle of viewControls.toggles) {
      toggle.checked = visible.has(toggle.dataset.viewToggle);
    }
    applyViewConfig(name);
  }

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
    const visible = visibleViewNames();
    for (const name of [...dirtyPanels]) {
      if (!visible.has(name)) {
        continue;
      }
      panelRenderers.get(name)();
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

  peOverviewControls.measure.addEventListener("change", () =>
    scheduleRender(["pe-grid"]),
  );
  for (const button of peOverviewControls.modes) {
    button.addEventListener("click", () => {
      setPeOverviewMode(button.dataset.peOverviewMode);
      scheduleRender(["pe-grid"]);
    });
  }

  function filtersChanged() {
    updateFilterSummaries();
    memoryLayoutCache.clear();
    memoryMetricsCache.clear();
    filterContextCache.clear();
    relationshipModelCache.clear();
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
  });
  relationshipControls.measure.addEventListener("change", () =>
    scheduleRender(["relationships"]),
  );
  relationshipControls.strength.addEventListener("input", () =>
    scheduleRender(["relationships"]),
  );
  skipMemoryGaps.addEventListener("change", () => {
    scheduleRender(["tensor-memory", "memory-details"]);
  });

  viewControls.layout.addEventListener("change", () => applyViewConfig());
  for (const toggle of viewControls.toggles) {
    toggle.addEventListener("change", () => applyViewConfig());
  }
  for (const button of viewControls.presets) {
    button.addEventListener("click", () =>
      setViewPreset(button.dataset.preset),
    );
  }

  // These callbacks are used by modules whose handlers are initialized below.
  Object.assign(App, {
    selectionChanged,
    filtersChanged,
    applyViewConfig,
    setViewPreset,
    flushRender: renderDirtyPanels,
  });
  initializeFilterControls();
  initializePeOverviewControls();
  updateRelationshipMeasureOptions();
  const restoredWorkspace = initializeWorkspace();
  if (restoredWorkspace) {
    applyViewConfig();
  } else {
    setViewPreset("summary");
  }
  renderWarnings();
})();
