// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    peOverviewControls,
    peChartColumnConfiguration,
    tensorOverviewControls,
    tensorOverviewColumnConfiguration,
    layerOverviewControls,
    layerOverviewColumnConfiguration,
    memoryOverviewControls,
    memoryOverviewColumnConfiguration,
    memoryVisualMode,
    skipMemoryGaps,
    relationshipControls,
    timetableGraphControls,
  } = App;

  function captureViewSettings() {
    return {
      peOverview: {
        chartOrder: peOverviewControls.chartOrder,
        chartSortKey: peOverviewControls.chartSortKey,
        chartSortDirection: peOverviewControls.chartSortDirection,
        chartColumns: peChartColumnConfiguration.snapshot(),
        gridEncoding: peOverviewControls.gridEncoding,
        measure: peOverviewControls.measure.value,
        mode: peOverviewControls.mode,
        scaleMode: peOverviewControls.scaleMode,
        fixedMaximum: peOverviewControls.fixedMaximum.value,
      },
      tensorOverview: {
        sortKey: tensorOverviewControls.sortKey,
        sortDirection: tensorOverviewControls.sortDirection,
        pageSize: tensorOverviewControls.pageSize,
        columns: tensorOverviewColumnConfiguration.snapshot(),
      },
      layerOverview: {
        sortKey: layerOverviewControls.sortKey,
        sortDirection: layerOverviewControls.sortDirection,
        columns: layerOverviewColumnConfiguration.snapshot(),
      },
      memoryOverview: {
        sortKey: memoryOverviewControls.sortKey,
        sortDirection: memoryOverviewControls.sortDirection,
        columns: memoryOverviewColumnConfiguration.snapshot(),
      },
      memoryVisualMode: memoryVisualMode.value,
      skipMemoryGaps: skipMemoryGaps.checked,
      relationshipLimit: relationshipControls.limit.value,
      timetableGraph: {
        tensorColour: timetableGraphControls.tensorColour.value,
        groupSize: timetableGraphControls.groupSize.value,
        groupRows: timetableGraphControls.groupRows.value,
        tensorScale: timetableGraphControls.tensorScale.value,
        computeScale: timetableGraphControls.computeScale.value,
        laneMode: timetableGraphControls.computeLanes.value,
        edgeDisplay:
          timetableGraphControls.edgesAll.getAttribute("aria-pressed") ===
          "true"
            ? "all"
            : "selection",
      },
      relationships: {
        mode: relationshipControls.mode.value,
        measure: relationshipControls.measure.value,
        strength: relationshipControls.strength.value,
      },
    };
  }

  function restoreViewSettings(config) {
    const currentGraph = captureViewSettings().timetableGraph;
    const measure = config.peOverview?.measure;
    if (
      [...peOverviewControls.measure.options].some(
        (option) => option.value === measure,
      )
    ) {
      peOverviewControls.measure.value = measure;
    }
    App.setPeGridEncoding(config.peOverview?.gridEncoding || "colour");
    App.setPeChartOrder(config.peOverview?.chartOrder || "pe-asc");
    peChartColumnConfiguration.restore(config.peOverview?.chartColumns);
    if (config.peOverview?.chartSortKey) {
      App.setPeChartSort(
        config.peOverview.chartSortKey,
        config.peOverview.chartSortDirection,
      );
    }
    App.setPeScaleMode(config.peOverview?.scaleMode || "global");
    peOverviewControls.fixedMaximum.value =
      config.peOverview?.fixedMaximum || peOverviewControls.fixedMaximum.value;
    App.setPeOverviewMode(config.peOverview?.mode || "grid");
    App.setTensorOverviewSort(
      config.tensorOverview?.sortKey || "read-ratio",
      config.tensorOverview?.sortDirection || "desc",
    );
    App.setTensorOverviewPageSize(config.tensorOverview?.pageSize || 200);
    tensorOverviewColumnConfiguration.restore(config.tensorOverview?.columns);
    layerOverviewColumnConfiguration.restore(config.layerOverview?.columns);
    App.setLayerOverviewSort(
      config.layerOverview?.sortKey || "layer",
      config.layerOverview?.sortDirection || "asc",
    );
    memoryOverviewColumnConfiguration.restore(config.memoryOverview?.columns);
    App.setMemoryOverviewSort(
      config.memoryOverview?.sortKey || "memory",
      config.memoryOverview?.sortDirection || "asc",
    );
    memoryVisualMode.value = config.memoryVisualMode || "combined";
    skipMemoryGaps.checked = config.skipMemoryGaps ?? true;
    relationshipControls.limit.value = config.relationshipLimit || "500";
    const graph = config.timetableGraph || {};
    for (const kind of ["tensor", "compute"]) {
      App.setTimetableGraphNodeScale(kind, graph[`${kind}Scale`] ?? 1);
    }
    if (graph.laneMode === undefined && graph.computeLanes !== undefined)
      graph.laneMode = graph.computeLanes ? "operation" : "none";
    const graphSetters = {
      tensorColour: App.setTimetableGraphTensorColourMode,
      groupSize: App.setTimetableGraphGroupSizeMode,
      groupRows: App.setTimetableGraphGroupRows,
      laneMode: App.setTimetableGraphComputeLaneMode,
      edgeDisplay: App.setTimetableGraphEdgeDisplayMode,
    };
    for (const [key, setter] of Object.entries(graphSetters)) {
      if (graph[key] !== undefined && graph[key] !== currentGraph[key])
        setter(graph[key]);
    }

    if (config.relationships) {
      relationshipControls.mode.value = config.relationships.mode;
      App.updateRelationshipMeasureOptions();
      relationshipControls.measure.value = config.relationships.measure;
      relationshipControls.strength.value = config.relationships.strength;
      relationshipControls.strengthValue.textContent =
        relationshipControls.strength.value;
    }
  }

  Object.assign(App, { captureViewSettings, restoreViewSettings });
})();
