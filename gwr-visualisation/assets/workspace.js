// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const { viewControls, workspaceDefinitions, summaryViewIds } = App;
  const shell = document.getElementById("workspace-shell");
  const storageKey = `gwr-visualisation-workspace-v2:${App.data.summary.timetable}`;
  const legacyKey = `gwr-visualisation-workspace-v1:${App.data.summary.timetable}`;
  const details = new Set([
    "selected-tensor",
    "selected-node",
    "layer-details",
    "selected-pe",
    "memory-details",
  ]);
  const registry = new Map();
  const regions = {};
  const visible = new Set();
  let saved, defaults;
  let ready = false;
  let restoring = false;
  let focused = null;
  let narrowActive = null;
  let layoutFrame = null;
  let effectiveLayout = "tabs";
  let renderedInspectorKind;
  const work = document.createElement("div");
  work.className = "workspace-work";
  const split = document.createElement("div");
  split.className = "workspace-divider";
  const inspectorSplit = document.createElement("div");
  inspectorSplit.className = "workspace-divider workspace-inspector-divider";
  const inspectorContent = document.createElement("div");
  inspectorContent.id = "workspace-inspector-content";
  const inspectorEmpty = document.createElement("p");
  inspectorEmpty.className = "workspace-empty";
  inspectorEmpty.textContent = "No matching selection.";
  inspectorContent.append(inspectorEmpty);

  function config() {
    return saved.workspaces[saved.active];
  }
  function button(label, callback) {
    const element = document.createElement("button");
    element.type = "button";
    element.textContent = label;
    element.addEventListener("click", callback);
    return element;
  }

  function createRegion(name) {
    const root = document.createElement("section");
    root.className = `workspace-region workspace-${name}`;
    root.setAttribute(
      "aria-label",
      name === "inspector" ? "Inspector" : `${name} views`,
    );
    const bar = document.createElement("div");
    bar.className = "workspace-tabs";
    const tabs = document.createElement("div");
    tabs.className = "workspace-tab-list";
    tabs.setAttribute("role", "tablist");
    tabs.setAttribute("aria-label", `${name} views`);
    const tools = document.createElement("div");
    tools.className = "workspace-region-tools";
    const body = document.createElement("div");
    body.className = "workspace-region-body";
    bar.append(tabs, tools);
    root.append(bar, body);
    return { root, tabs, tools, body };
  }

  function readStorage() {
    try {
      return JSON.parse(
        localStorage.getItem(storageKey) || localStorage.getItem(legacyKey),
      );
    } catch {
      return null;
    }
  }

  function workspaceChanged() {
    if (!ready || restoring) return;
    config().settings = App.captureViewSettings();
    try {
      localStorage.setItem(storageKey, JSON.stringify(saved));
    } catch {
      /* Reports also work with browser storage disabled. */
    }
  }

  function workspaceDefaults(name, base) {
    const settings = structuredClone(base);
    settings.peOverview.measure =
      name === "tensors"
        ? "data:read"
        : name === "memory"
          ? "data:total"
          : "compute:machine-ops";
    settings.peOverview.mode = ["compute", "memory"].includes(name)
      ? "chart"
      : "grid";
    settings.relationships.mode =
      name === "memory"
        ? "memory"
        : name === "dataflow"
          ? "tensor-pe"
          : "compute";
    settings.relationships.measure = ["memory", "dataflow"].includes(name)
      ? "read"
      : "machine-ops";
    return settings;
  }

  function switchWorkspace(name) {
    if (!Object.hasOwn(workspaceDefinitions, name)) return;
    workspaceChanged();
    saved.active = name;
    focused = null;
    narrowActive = config().activePrimary;
    restoreSettings();
    App.markAllViewsDirty();
    applyWorkspace();
    workspaceChanged();
  }

  function restoreSettings() {
    restoring = true;
    try {
      App.restoreViewSettings({
        ...defaults[saved.active],
        ...config().settings,
      });
    } catch {
      config().settings = {};
      App.restoreViewSettings(defaults[saved.active]);
    }
    if (saved.active === "memory" && !config().settings.peOverview)
      App.peChartColumnConfiguration.apply(["data:read", "data:write"]);
    restoring = false;
  }

  function activateView(id) {
    if (id === "inspector") config().inspector = true;
    else {
      const side = App.workspaceRegionFor(config(), id);
      if (side)
        config()[side === "primary" ? "activePrimary" : "activeCompanion"] = id;
    }
    narrowActive = id;
    focused = focused ? id : null;
    applyWorkspace();
    workspaceChanged();
  }

  function tabMarkup(region, ids, active) {
    const tabs = regions[region].tabs;
    tabs.replaceChildren();
    ids.forEach((id, index) => {
      const tab = button(
        id === "inspector" ? "Inspector" : registry.get(id).label,
        () => activateView(id),
      );
      tab.setAttribute("role", "tab");
      tab.id = `workspace-tab-${region}-${id}`;
      tab.setAttribute("aria-selected", String(id === active));
      tab.setAttribute(
        "aria-controls",
        id === "inspector" ? inspectorContent.id : registry.get(id).root.id,
      );
      tab.tabIndex = id === active ? 0 : -1;
      tab.addEventListener("keydown", (event) => {
        if (
          id === active &&
          App.overviewNavigationKeys.includes(event.key) &&
          !event.altKey &&
          !event.ctrlKey &&
          !event.metaKey &&
          !event.shiftKey
        ) {
          const overview = registry
            .get(id)
            ?.root.querySelector('[aria-keyshortcuts~="PageDown"]');
          if (overview && overview.offsetParent) {
            event.preventDefault();
            overview.focus({ preventScroll: true });
            overview.dispatchEvent(
              new KeyboardEvent("keydown", { key: event.key, bubbles: true }),
            );
            return;
          }
        }
        const target =
          event.key === "Home"
            ? 0
            : event.key === "End"
              ? ids.length - 1
              : event.key === "ArrowRight"
                ? (index + 1) % ids.length
                : event.key === "ArrowLeft"
                  ? (index + ids.length - 1) % ids.length
                  : null;
        if (target === null) return;
        event.preventDefault();
        activateView(ids[target]);
        document
          .getElementById(`workspace-tab-${region}-${ids[target]}`)
          ?.focus();
      });
      tabs.append(tab);
    });
  }

  function regionTools(region, active) {
    const tools = regions[region].tools;
    tools.replaceChildren();
    const menu = document.createElement("details");
    const trigger = document.createElement("summary");
    trigger.textContent = "Views";
    const popup = document.createElement("div");
    popup.className = "workspace-menu";
    const select = document.createElement("select");
    select.setAttribute("aria-label", `Add view to ${region}`);
    for (const [id, view] of registry)
      if (!view.detail) select.append(App.option(id, view.label));
    popup.append(
      select,
      button("Add view", () => {
        App.openWorkspaceView(config(), region, select.value);
        activateView(select.value);
      }),
    );
    if (active && active !== "inspector") {
      for (const [action, label] of [
        ["earlier", "Move tab earlier"],
        ["later", "Move tab later"],
        ["move", "Move to other region"],
        ["close", "Close tab"],
      ]) {
        const command = button(label, () => {
          if (App.changeWorkspaceTab(config(), active, action)) {
            narrowActive = config().activePrimary;
            applyWorkspace();
            workspaceChanged();
          }
        });
        const side = App.workspaceRegionFor(config(), active);
        const index = config()[side].indexOf(active);
        command.disabled = ["close", "move"].includes(action)
          ? side === "primary" && config().primary.length === 1
          : action === "earlier"
            ? index === 0
            : index === config()[side].length - 1;
        popup.append(command);
      }
    }
    menu.append(trigger, popup);
    tools.append(menu);
    if (active)
      tools.append(
        button(focused ? "Restore" : "Maximize", () => {
          focused = focused ? null : active;
          applyWorkspace();
        }),
      );
  }

  function mount(id, destination) {
    const view = registry.get(id);
    if (view.root.parentElement !== destination) destination.append(view.root);
    view.root.hidden = false;
    visible.add(id);
    if (id === "summary")
      for (const name of summaryViewIds) mount(name, view.root);
  }

  function inspectorViews() {
    if (!App.state.activeEntity?.id) return [];
    const kind = App.state.activeEntity?.kind;
    return kind === "tensor"
      ? ["selected-tensor", "selected-node"]
      : kind === "layer"
        ? ["layer-details"]
        : kind === "pe"
          ? ["selected-pe"]
          : kind === "memory"
            ? ["memory-details"]
            : ["compute", "group"].includes(kind)
              ? ["selected-node"]
              : [];
  }

  function refreshWorkspaceInspector() {
    const kind = App.state.activeEntity?.id
      ? App.state.activeEntity.kind
      : "empty";
    inspectorContent.dataset.kind = kind;
    if (kind !== renderedInspectorKind) {
      renderedInspectorKind = kind;
      for (const id of details) {
        registry.get(id).root.hidden = true;
        visible.delete(id);
      }
      inspectorEmpty.hidden = inspectorViews().length > 0;
    }
    if (visible.has("inspector"))
      for (const id of inspectorViews()) mount(id, inspectorContent);
  }

  function mountInspector(destination) {
    if (inspectorContent.parentElement !== destination)
      destination.append(inspectorContent);
    inspectorContent.hidden = false;
    visible.add("inspector");
    refreshWorkspaceInspector();
  }

  function applyWorkspace() {
    if (!ready) return;
    const previous = new Set(visible);
    visible.clear();
    const c = config(),
      width = shell.clientWidth;
    effectiveLayout = App.workspaceArrangementFor(c, width, shell.clientHeight);
    const narrowInspector = width < 960,
      mergedTabs = effectiveLayout === "tabs";
    const ids = mergedTabs ? [...c.primary, ...c.companion] : [...c.primary];
    if (narrowInspector && c.inspector) ids.push("inspector");
    if (!ids.includes(narrowActive)) narrowActive = c.activePrimary;
    const primary = focused || (mergedTabs ? narrowActive : c.activePrimary);
    shell.dataset.arrangement = focused ? "single" : effectiveLayout;
    shell.dataset.inspector = String(
      c.inspector && !narrowInspector && !focused,
    );
    shell.dataset.companion = String(
      !focused && !mergedTabs && c.companion.length > 0,
    );
    for (const view of registry.values()) view.root.hidden = true;
    inspectorContent.hidden = true;
    if (primary === "inspector") mountInspector(regions.primary.body);
    else mount(primary, regions.primary.body);
    const companionVisible = shell.dataset.companion === "true";
    if (companionVisible) mount(c.activeCompanion, regions.companion.body);
    const inspectorVisible = shell.dataset.inspector === "true";
    if (inspectorVisible) mountInspector(regions.inspector.body);
    regions.companion.root.hidden = !companionVisible;
    regions.inspector.root.hidden = !inspectorVisible;
    tabMarkup("primary", focused ? [focused] : ids, primary);
    tabMarkup("companion", mergedTabs ? [] : c.companion, c.activeCompanion);
    regionTools("primary", primary);
    regionTools("companion", c.activeCompanion);
    updateDividers();
    document.getElementById("workspace-arrangement").value = c.arrangement;
    document
      .getElementById("workspace-inspector-toggle")
      .setAttribute("aria-pressed", String(c.inspector));
    document
      .querySelectorAll("[data-workspace]")
      .forEach((b) =>
        b.setAttribute(
          "aria-pressed",
          String(b.dataset.workspace === saved.active),
        ),
      );
    for (const id of visible) if (!previous.has(id)) App.markViewsDirty([id]);
    App.scheduleVisibleViews();
  }

  function updateDividers() {
    const c = config(),
      inspector = shell.dataset.inspector === "true",
      width = shell.clientWidth;
    const inspectorWidth = Math.min(
      c.inspectorWidth,
      Math.max(280, width - (effectiveLayout === "beside" ? 856 : 488)),
    );
    shell.style.setProperty("--inspector-width", `${inspectorWidth}px`);
    const available = Math.max(1, width - (inspector ? inspectorWidth + 8 : 0));
    const height = Math.max(1, shell.clientHeight);
    const ratio =
      effectiveLayout === "beside"
        ? Math.max(
            480 / available,
            Math.min(1 - 368 / available, c.horizontalRatio),
          )
        : Math.max(220 / height, Math.min(1 - 228 / height, c.verticalRatio));
    work.style.setProperty("--split-ratio", `${ratio * 100}%`);
    split.hidden = shell.dataset.companion !== "true";
    inspectorSplit.hidden = !inspector;
    split.setAttribute(
      "aria-orientation",
      effectiveLayout === "beside" ? "vertical" : "horizontal",
    );
    split.setAttribute("aria-valuenow", String(Math.round(ratio * 100)));
    inspectorSplit.setAttribute(
      "aria-valuenow",
      String(Math.round(inspectorWidth)),
    );
  }

  function bindDivider(element, inspector) {
    element.tabIndex = 0;
    element.setAttribute("role", "separator");
    element.setAttribute(
      "aria-label",
      inspector ? "Inspector width" : "Primary and companion split",
    );
    element.setAttribute("aria-orientation", "vertical");
    let drag = null;
    const value = () =>
      inspector
        ? config().inspectorWidth
        : config()[
            effectiveLayout === "beside" ? "horizontalRatio" : "verticalRatio"
          ];
    const change = (delta, initial) => {
      if (inspector)
        config().inspectorWidth = Math.max(
          280,
          Math.min(shell.clientWidth - 488, initial - delta),
        );
      else {
        const key =
          effectiveLayout === "beside" ? "horizontalRatio" : "verticalRatio";
        const size =
          effectiveLayout === "beside" ? work.clientWidth : work.clientHeight;
        config()[key] = Math.max(0.2, Math.min(0.8, initial + delta / size));
      }
      updateDividers();
    };
    element.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      element.setPointerCapture(event.pointerId);
      drag = { x: event.clientX, y: event.clientY, initial: value() };
    });
    element.addEventListener("pointermove", (event) => {
      if (drag)
        change(
          inspector || effectiveLayout === "beside"
            ? event.clientX - drag.x
            : event.clientY - drag.y,
          drag.initial,
        );
    });
    for (const type of ["pointerup", "pointercancel", "lostpointercapture"])
      element.addEventListener(type, () => {
        if (drag) {
          drag = null;
          workspaceChanged();
        }
      });
    element.addEventListener("keydown", (event) => {
      const delta = {
        ArrowLeft: -16,
        ArrowRight: 16,
        ArrowUp: -16,
        ArrowDown: 16,
      }[event.key];
      if (!delta) return;
      event.preventDefault();
      change(delta, value());
      workspaceChanged();
    });
    element.addEventListener("dblclick", () => {
      if (inspector) config().inspectorWidth = 360;
      else {
        config().horizontalRatio = 0.5;
        config().verticalRatio = 0.5;
      }
      updateDividers();
      workspaceChanged();
    });
  }

  function compactGraphControls() {
    const controls = App.timetableGraph
      .querySelector("canvas")
      .closest("[data-view]")
      .querySelector(".timetable-graph-controls");
    const options = document.createElement("details");
    options.className = "workspace-graph-options";
    const title = document.createElement("summary");
    title.textContent = "Graph settings";
    const content = document.createElement("div");
    content.className = "workspace-graph-options-body";
    for (const child of [...controls.children]) {
      if (child.matches("label, .timetable-graph-edge-mode"))
        content.append(child);
    }
    options.append(title, content);
    controls.prepend(options);
    const panel = controls.closest("[data-view]");
    const sizeMenu = () => {
      if (!options.open || panel.hidden) return;
      const available =
        panel.getBoundingClientRect().bottom -
        content.getBoundingClientRect().top -
        8;
      content.style.maxHeight = `${Math.max(0, available)}px`;
    };
    options.addEventListener("toggle", sizeMenu);
    const menuObserver = new ResizeObserver(sizeMenu);
    menuObserver.observe(panel);
    menuObserver.observe(controls);
    const legend = controls
      .closest("[data-view]")
      .querySelector(".timetable-graph-legend");
    const legendDetails = document.createElement("details");
    legendDetails.className = "workspace-legend";
    const legendTitle = document.createElement("summary");
    legendTitle.textContent = "Legend";
    legend.replaceWith(legendDetails);
    legendDetails.append(legendTitle, legend);
  }

  function initializeWorkspace(renderers, dependencies) {
    compactGraphControls();
    for (const panel of viewControls.panels) {
      const id = panel.dataset.view;
      panel.id ||= `workspace-view-${id}`;
      registry.set(id, {
        root: panel,
        label: panel.querySelector("h2").textContent,
        detail: details.has(id),
        render: renderers.get(id),
        minimumWidth: details.has(id) ? 280 : 480,
        minimumHeight: 220,
        dependencies: Object.keys(dependencies).filter((kind) =>
          dependencies[kind].includes(id),
        ),
        captureSettings: App.captureViewSettings,
        restoreSettings: App.restoreViewSettings,
      });
    }
    const summary = document.createElement("section");
    summary.id = "workspace-view-summary";
    summary.className = "workspace-summary";
    registry.set("summary", {
      root: summary,
      label: "All summaries",
      detail: false,
      minimumWidth: 320,
      minimumHeight: 220,
    });
    viewControls.views.append(summary);
    for (const name of ["primary", "companion", "inspector"])
      regions[name] = createRegion(name);
    regions.inspector.tabs.textContent = "Inspector";
    regions.inspector.tabs.removeAttribute("role");
    work.append(regions.primary.root, split, regions.companion.root);
    shell.append(work, inspectorSplit, regions.inspector.root);
    bindDivider(split, false);
    bindDivider(inspectorSplit, true);
    const baseline = App.captureViewSettings();
    defaults = Object.fromEntries(
      Object.keys(workspaceDefinitions).map((name) => [
        name,
        workspaceDefaults(name, baseline),
      ]),
    );
    saved = App.restoreWorkspaceModel(
      readStorage(),
      new Set([...registry].filter(([, v]) => !v.detail).map(([id]) => id)),
    );
    const navigation = document.getElementById("workspace-navigation");
    for (const [name, definition] of Object.entries(workspaceDefinitions)) {
      const tab = button(definition.label, () => switchWorkspace(name));
      tab.dataset.workspace = name;
      navigation.append(tab);
    }
    document
      .getElementById("workspace-arrangement")
      .addEventListener("change", (event) => {
        config().arrangement = event.target.value;
        applyWorkspace();
        workspaceChanged();
      });
    document
      .getElementById("workspace-inspector-toggle")
      .addEventListener("click", () => {
        config().inspector = !config().inspector;
        if (config().inspector && shell.clientWidth < 960)
          narrowActive = "inspector";
        applyWorkspace();
        workspaceChanged();
      });
    document.getElementById("workspace-reset").addEventListener("click", () => {
      saved.workspaces[saved.active] = App.createWorkspace(saved.active);
      restoreSettings();
      focused = null;
      narrowActive = null;
      App.markAllViewsDirty();
      applyWorkspace();
      workspaceChanged();
    });
    restoreSettings();
    ready = true;
    applyWorkspace();
    const observer = new ResizeObserver(() => {
      if (layoutFrame !== null) return;
      layoutFrame = requestAnimationFrame(() => {
        layoutFrame = null;
        applyWorkspace();
      });
    });
    observer.observe(shell);
    const contentObserver = new ResizeObserver((entries) => {
      const names = entries
        .map((entry) => entry.target.dataset.view)
        .filter((name) => visible.has(name));
      if (names.length) {
        App.markViewsDirty(names);
        App.scheduleVisibleViews();
      }
    });
    for (const view of registry.values())
      if (view.root.dataset.view) contentObserver.observe(view.root);
  }

  Object.assign(App, {
    viewRegistry: registry,
    initializeWorkspace,
    workspaceChanged,
    refreshWorkspaceInspector,
    workspaceVisibleViews: () => new Set(visible),
  });
})();
