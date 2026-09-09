// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const definitions = {
    summary: { label: "Summary", primary: ["summary"], companion: [] },
    layers: {
      label: "Layers",
      primary: ["layer-summary"],
      companion: ["relationships", "pe-grid"],
    },
    dataflow: {
      label: "Dataflow",
      primary: ["timetable-graph"],
      companion: ["relationships", "layer-summary"],
    },
    compute: {
      label: "Compute",
      primary: ["pe-grid"],
      companion: ["relationships", "layer-summary"],
    },
    memory: {
      label: "Memory",
      primary: ["memories-overview"],
      companion: ["tensor-memory", "pe-grid", "relationships"],
    },
    tensors: {
      label: "Tensors",
      primary: ["tensor-overview"],
      companion: ["tensor-accesses", "tensor-memory", "pe-grid"],
    },
  };
  const summaries = ["timetable-summary", "compute-summary", "memory-summary"];

  function createWorkspace(name, settings = {}) {
    const definition = definitions[name];
    return {
      primary: [...definition.primary],
      companion: [...definition.companion],
      activePrimary: definition.primary[0],
      activeCompanion: definition.companion[0] || null,
      arrangement: "auto",
      horizontalRatio: 0.5,
      verticalRatio: 0.5,
      inspectorWidth: 360,
      inspector: name !== "summary",
      settings: structuredClone(settings),
    };
  }

  function regionFor(config, id) {
    return config.primary.includes(id)
      ? "primary"
      : config.companion.includes(id)
        ? "companion"
        : null;
  }

  function activeKey(region) {
    return region === "primary" ? "activePrimary" : "activeCompanion";
  }

  function openView(config, region, id) {
    const existing = regionFor(config, id);
    if (existing) {
      config[activeKey(existing)] = id;
      return existing;
    }
    // Combined summaries and their individual views share the same DOM roots.
    const conflicts =
      id === "summary" ? summaries : summaries.includes(id) ? ["summary"] : [];
    for (const side of ["primary", "companion"]) {
      config[side] = config[side].filter((view) => !conflicts.includes(view));
    }
    config[region].push(id);
    config[activeKey(region)] = id;
    repairTabs(config);
    return regionFor(config, id);
  }

  function repairTabs(config) {
    if (!config.primary.length && config.companion.length)
      config.primary.push(config.companion.shift());
    for (const region of ["primary", "companion"]) {
      if (!config[region].includes(config[activeKey(region)]))
        config[activeKey(region)] = config[region][0] || null;
    }
  }

  function changeTab(config, id, action) {
    const region = regionFor(config, id);
    if (!region) return false;
    const tabs = config[region];
    const index = tabs.indexOf(id);
    if (action === "close") {
      if (region === "primary" && tabs.length === 1) return false;
      tabs.splice(index, 1);
    } else if (action === "move") {
      if (region === "primary" && tabs.length === 1) return false;
      tabs.splice(index, 1);
      const destination = region === "primary" ? "companion" : "primary";
      config[destination].push(id);
      config[activeKey(destination)] = id;
    } else {
      const target = index + (action === "earlier" ? -1 : 1);
      if (target < 0 || target >= tabs.length) return false;
      [tabs[index], tabs[target]] = [tabs[target], tabs[index]];
    }
    repairTabs(config);
    return true;
  }

  function arrangementFor(config, width, height) {
    if (width < 960) return "tabs";
    const available = width - (config.inspector ? 280 + 8 : 0);
    if (!config.companion.length) return "single";
    const requested =
      config.arrangement === "auto"
        ? width >= 1.5 * height && available >= 848
          ? "beside"
          : "below"
        : config.arrangement;
    if (requested === "beside") return available >= 848 ? "beside" : "tabs";
    return height >= 600 && available >= 480 ? "below" : "tabs";
  }

  function restoreWorkspaceModel(saved, validIds) {
    const result = { version: 2, active: "summary", workspaces: {} };
    for (const name of Object.keys(definitions)) {
      const config = createWorkspace(name);
      const source = saved?.version === 2 ? saved.workspaces?.[name] : null;
      if (source && typeof source === "object") {
        const seen = new Set();
        for (const region of ["primary", "companion"]) {
          const values = Array.isArray(source[region])
            ? source[region]
            : config[region];
          config[region] = values.filter((id) => {
            if (!validIds.has(id) || seen.has(id)) return false;
            if (id === "summary" && summaries.some((view) => seen.has(view)))
              return false;
            if (summaries.includes(id) && seen.has("summary")) return false;
            seen.add(id);
            return true;
          });
          config[activeKey(region)] = source[activeKey(region)];
        }
        if (!config.primary.length && !config.companion.length)
          config.primary = [...definitions[name].primary];
        repairTabs(config);
        if (["auto", "beside", "below"].includes(source.arrangement))
          config.arrangement = source.arrangement;
        for (const key of ["horizontalRatio", "verticalRatio"]) {
          if (Number.isFinite(source[key]))
            config[key] = Math.max(0.2, Math.min(0.8, source[key]));
        }
        if (Number.isFinite(source.inspectorWidth))
          config.inspectorWidth = Math.max(
            280,
            Math.min(600, source.inspectorWidth),
          );
        if (typeof source.inspector === "boolean")
          config.inspector = source.inspector;
        if (source.settings && typeof source.settings === "object")
          config.settings = structuredClone(source.settings);
      } else if (saved?.version === 1) {
        for (const key of [
          "peOverview",
          "tensorOverview",
          "layerOverview",
          "memoryOverview",
          "memoryVisualMode",
          "skipMemoryGaps",
          "relationshipLimit",
          "timetableGraph",
        ]) {
          if (saved[key] !== undefined)
            config.settings[key] = structuredClone(saved[key]);
        }
      }
      result.workspaces[name] = config;
    }
    if (saved?.version === 2 && Object.hasOwn(definitions, saved.active))
      result.active = saved.active;
    return result;
  }

  Object.assign(window.GWR_VISUALISATION_APP, {
    workspaceDefinitions: definitions,
    summaryViewIds: summaries,
    createWorkspace,
    workspaceRegionFor: regionFor,
    openWorkspaceView: openView,
    changeWorkspaceTab: changeTab,
    workspaceArrangementFor: arrangementFor,
    restoreWorkspaceModel,
  });
})();
