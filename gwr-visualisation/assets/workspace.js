// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const { data, viewControls, retainedFocus } = App;

  const WORKSPACE_VERSION = 1;
  const storageKey = `gwr-visualisation-workspace-v${WORKSPACE_VERSION}:${data.summary.timetable}`;
  const initialPanelOrder = viewControls.panels.map(
    (panel) => panel.dataset.view,
  );
  const defaultFullWidthPanels = new Set([
    "layer-summary",
    "relationships",
    "tensor-memory",
  ]);
  let workspaceReady = false;
  let focusedPanel = null;
  let draggedPanel = null;

  function panelToggle(panel) {
    return viewControls.toggles.find(
      (toggle) => toggle.dataset.viewToggle === panel.dataset.view,
    );
  }

  function panelLabel(panel) {
    return (
      panel.dataset.workspaceLabel ||
      panel.querySelector("h2")?.textContent.trim() ||
      panel.dataset.view
    );
  }

  function applyPanelWidth(panel, width) {
    panel.dataset.workspaceWidth = width;
    panel.style.gridColumn = width === "full" ? "1 / -1" : `span ${width}`;
    const select = panel.querySelector(".workspace-panel-width");
    if (select) {
      select.value = width;
    }
  }

  function setPanelCollapsed(panel, collapsed) {
    panel.classList.toggle("workspace-collapsed", collapsed);
    const button = panel.querySelector(".workspace-collapse");
    if (button) {
      button.textContent = collapsed ? "Expand" : "Collapse";
      button.setAttribute("aria-expanded", collapsed ? "false" : "true");
    }
  }

  function updateFocusButtons() {
    for (const panel of viewControls.panels) {
      const button = panel.querySelector(".workspace-focus");
      if (button) {
        const active = panel === focusedPanel;
        button.textContent = active ? "Exit focus" : "Focus";
        button.setAttribute("aria-pressed", active ? "true" : "false");
      }
    }
  }

  function setFocusedPanel(panel) {
    focusedPanel = focusedPanel === panel ? null : panel;
    viewControls.views.classList.toggle(
      "workspace-has-focus",
      Boolean(focusedPanel),
    );
    for (const candidate of viewControls.panels) {
      candidate.classList.toggle(
        "workspace-focused",
        candidate === focusedPanel,
      );
    }
    updateFocusButtons();
  }

  function reconcileWorkspaceFocus(visibleViews) {
    if (
      focusedPanel &&
      retainedFocus(focusedPanel.dataset.view, visibleViews) === null
    ) {
      setFocusedPanel(focusedPanel);
    }
  }

  function movePanel(panel, direction) {
    const visible = [...viewControls.views.children].filter(
      (candidate) => !candidate.hidden,
    );
    const index = visible.indexOf(panel);
    const target = visible[index + direction];
    if (!target) {
      return;
    }
    if (direction < 0) {
      viewControls.views.insertBefore(panel, target);
    } else {
      viewControls.views.insertBefore(panel, target.nextSibling);
    }
    saveWorkspace();
  }

  function setPanelVisible(panel, visible) {
    const toggle = panelToggle(panel);
    if (toggle) {
      toggle.checked = visible;
    }
    if (!visible && panel === focusedPanel) {
      setFocusedPanel(panel);
    }
    App.applyViewConfig();
  }

  function decoratePanel(panel) {
    panel.dataset.workspaceLabel = panelLabel(panel);
    const heading = panel.querySelector(
      ":scope > h2, :scope > .panel-title-row h2",
    );
    const bar = document.createElement("div");
    bar.className = "workspace-panel-bar";
    if (heading) {
      bar.append(heading);
    } else {
      const title = document.createElement("h2");
      title.textContent = panel.dataset.workspaceLabel;
      bar.append(title);
    }

    const tools = document.createElement("div");
    tools.className = "workspace-panel-tools";

    const drag = document.createElement("button");
    drag.type = "button";
    drag.className = "workspace-drag";
    drag.textContent = "Move";
    drag.draggable = true;
    drag.setAttribute("aria-label", `Drag ${panel.dataset.workspaceLabel}`);

    const up = document.createElement("button");
    up.type = "button";
    up.textContent = "Up";
    up.setAttribute(
      "aria-label",
      `Move ${panel.dataset.workspaceLabel} earlier`,
    );
    up.addEventListener("click", () => movePanel(panel, -1));

    const down = document.createElement("button");
    down.type = "button";
    down.textContent = "Down";
    down.setAttribute(
      "aria-label",
      `Move ${panel.dataset.workspaceLabel} later`,
    );
    down.addEventListener("click", () => movePanel(panel, 1));

    const width = document.createElement("select");
    width.className = "workspace-panel-width";
    width.setAttribute("aria-label", `${panel.dataset.workspaceLabel} width`);
    for (const [value, label] of [
      ["1", "1 column"],
      ["2", "2 columns"],
      ["full", "Full row"],
    ]) {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      width.append(option);
    }
    width.addEventListener("change", () => {
      applyPanelWidth(panel, width.value);
      saveWorkspace();
    });

    const collapse = document.createElement("button");
    collapse.type = "button";
    collapse.className = "workspace-collapse";
    collapse.addEventListener("click", () => {
      setPanelCollapsed(
        panel,
        !panel.classList.contains("workspace-collapsed"),
      );
      saveWorkspace();
    });

    const focus = document.createElement("button");
    focus.type = "button";
    focus.className = "workspace-focus";
    focus.textContent = "Focus";
    focus.setAttribute("aria-pressed", "false");
    focus.addEventListener("click", () => setFocusedPanel(panel));

    const hide = document.createElement("button");
    hide.type = "button";
    hide.textContent = "Hide";
    hide.setAttribute("aria-label", `Hide ${panel.dataset.workspaceLabel}`);
    hide.addEventListener("click", () => setPanelVisible(panel, false));

    drag.addEventListener("dragstart", (event) => {
      draggedPanel = panel;
      panel.classList.add("workspace-dragging");
      event.dataTransfer.effectAllowed = "move";
      event.dataTransfer.setData("text/plain", panel.dataset.view);
    });
    drag.addEventListener("dragend", () => {
      panel.classList.remove("workspace-dragging");
      draggedPanel = null;
      saveWorkspace();
    });
    panel.addEventListener("dragover", (event) => {
      if (!draggedPanel || draggedPanel === panel) {
        return;
      }
      event.preventDefault();
      const panels = [...viewControls.views.children];
      if (panels.indexOf(draggedPanel) < panels.indexOf(panel)) {
        viewControls.views.insertBefore(draggedPanel, panel.nextSibling);
      } else {
        viewControls.views.insertBefore(draggedPanel, panel);
      }
    });
    panel.addEventListener("pointerup", () => {
      if (panel.style.height) {
        saveWorkspace();
      }
    });

    tools.append(drag, up, down, width, collapse, focus, hide);
    bar.append(tools);
    panel.prepend(bar);
    applyPanelWidth(
      panel,
      defaultFullWidthPanels.has(panel.dataset.view) ? "full" : "1",
    );
    setPanelCollapsed(panel, false);
  }

  function workspaceSnapshot() {
    return {
      version: WORKSPACE_VERSION,
      layout: viewControls.layout.value,
      visible: viewControls.toggles
        .filter((toggle) => toggle.checked)
        .map((toggle) => toggle.dataset.viewToggle),
      order: [...viewControls.views.children].map(
        (panel) => panel.dataset.view,
      ),
      panels: Object.fromEntries(
        viewControls.panels.map((panel) => [
          panel.dataset.view,
          {
            width: panel.dataset.workspaceWidth || "1",
            height: panel.style.height || null,
            collapsed: panel.classList.contains("workspace-collapsed"),
          },
        ]),
      ),
    };
  }

  function saveWorkspace() {
    if (!workspaceReady) {
      return;
    }
    try {
      localStorage.setItem(storageKey, JSON.stringify(workspaceSnapshot()));
    } catch {
      // Static reports remain usable when storage is unavailable.
    }
    updateAddViewOptions();
  }

  function readWorkspace() {
    try {
      const value = JSON.parse(localStorage.getItem(storageKey));
      return value?.version === WORKSPACE_VERSION ? value : null;
    } catch {
      return null;
    }
  }

  function restoreWorkspace(config) {
    if (!config) {
      return false;
    }
    const legacyMemorySummary = !Object.prototype.hasOwnProperty.call(
      config.panels || {},
      "memories-overview",
    );
    const panelByName = new Map(
      viewControls.panels.map((panel) => [panel.dataset.view, panel]),
    );
    for (const name of config.order || []) {
      const restoredName = name === "compute-balance" ? "pe-grid" : name;
      const panel = panelByName.get(restoredName);
      if (panel) {
        viewControls.views.append(panel);
        panelByName.delete(restoredName);
        if (legacyMemorySummary && restoredName === "memory-summary") {
          const overview = panelByName.get("memories-overview");
          if (overview) {
            viewControls.views.append(overview);
            panelByName.delete("memories-overview");
          }
        }
      }
    }
    for (const panel of panelByName.values()) {
      viewControls.views.append(panel);
    }
    const configuredVisible = config.visible || [];
    const visible = new Set(
      configuredVisible.map((name) =>
        name === "compute-balance" ? "pe-grid" : name,
      ),
    );
    if (legacyMemorySummary && visible.has("memory-summary")) {
      visible.add("memories-overview");
    }
    for (const toggle of viewControls.toggles) {
      toggle.checked = visible.has(toggle.dataset.viewToggle);
    }
    if (
      [...viewControls.layout.options].some(
        (option) => option.value === config.layout,
      )
    ) {
      viewControls.layout.value = config.layout;
    }
    for (const panel of viewControls.panels) {
      const useLegacyBalance =
        panel.dataset.view === "pe-grid" &&
        configuredVisible.includes("compute-balance") &&
        !configuredVisible.includes("pe-grid");
      const panelConfig =
        (useLegacyBalance ? config.panels?.["compute-balance"] : null) ||
        config.panels?.[panel.dataset.view] ||
        {};
      applyPanelWidth(panel, panelConfig.width || "1");
      panel.style.height = panelConfig.height || "";
      setPanelCollapsed(panel, Boolean(panelConfig.collapsed));
    }
    return true;
  }

  function updateAddViewOptions() {
    const hiddenPanels = viewControls.panels.filter(
      (panel) => !panelToggle(panel)?.checked,
    );
    viewControls.addView.innerHTML = "";
    if (!hiddenPanels.length) {
      const option = document.createElement("option");
      option.textContent = "All panels visible";
      option.value = "";
      viewControls.addView.append(option);
      viewControls.addButton.disabled = true;
      return;
    }
    for (const panel of hiddenPanels) {
      const option = document.createElement("option");
      option.value = panel.dataset.view;
      option.textContent = panel.dataset.workspaceLabel;
      viewControls.addView.append(option);
    }
    viewControls.addButton.disabled = false;
  }

  function resetWorkspace() {
    try {
      localStorage.removeItem(storageKey);
    } catch {
      // Ignore unavailable storage.
    }
    focusedPanel = null;
    viewControls.views.classList.remove("workspace-has-focus");
    const panelByName = new Map(
      viewControls.panels.map((panel) => [panel.dataset.view, panel]),
    );
    for (const name of initialPanelOrder) {
      viewControls.views.append(panelByName.get(name));
    }
    for (const panel of viewControls.panels) {
      panel.style.height = "";
      panel.classList.remove("workspace-focused");
      applyPanelWidth(
        panel,
        defaultFullWidthPanels.has(panel.dataset.view) ? "full" : "1",
      );
      setPanelCollapsed(panel, false);
    }
    viewControls.layout.value = "one";
    updateFocusButtons();
    App.setViewPreset("summary");
  }

  function initializeWorkspace() {
    for (const panel of viewControls.panels) {
      decoratePanel(panel);
    }
    viewControls.addButton.addEventListener("click", () => {
      const panel = viewControls.panels.find(
        (candidate) => candidate.dataset.view === viewControls.addView.value,
      );
      if (panel) {
        setPanelVisible(panel, true);
      }
    });
    viewControls.resetButton.addEventListener("click", resetWorkspace);
    const restored = restoreWorkspace(readWorkspace());
    workspaceReady = true;
    updateAddViewOptions();
    return restored;
  }

  function workspaceChanged() {
    updateAddViewOptions();
    saveWorkspace();
  }

  Object.assign(App, {
    initializeWorkspace,
    workspaceChanged,
    reconcileWorkspaceFocus,
  });
})();
