// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = (window.GWR_VISUALISATION_APP = {});

  const data = window.GWR_VISUALISATION_DATA;
  const fmt = new Intl.NumberFormat("en");
  const compactCountFormat = new Intl.NumberFormat("en", {
    notation: "compact",
    maximumFractionDigits: 2,
  });

  const controls = {
    layer: document.getElementById("layer-filter"),
    pe: document.getElementById("pe-filter"),
    memory: document.getElementById("memory-filter"),
    tensor: document.getElementById("tensor-filter"),
    layerSummary: document.getElementById("layer-filter-summary"),
    peSummary: document.getElementById("pe-filter-summary"),
    memorySummary: document.getElementById("memory-filter-summary"),
    tensorSummary: document.getElementById("tensor-filter-summary"),
  };
  const viewControls = {
    views: document.getElementById("views"),
    panels: [...document.querySelectorAll("[data-view]")],
  };
  const grid = document.getElementById("pe-grid");
  const rowAxis = document.getElementById("row-axis");
  const colAxis = document.getElementById("col-axis");
  const selectedPanel = document.getElementById("selected-pe");
  const timetableSummary = document.getElementById("timetable-summary");
  const layerSummary = document.getElementById("layer-summary");
  const layerDetail = document.getElementById("layer-detail");
  const relationshipBundle = document.getElementById("relationship-bundle");
  const relationshipControls = {
    mode: document.getElementById("relationship-mode"),
    measure: document.getElementById("relationship-measure"),
    limit: document.getElementById("relationship-limit"),
    strength: document.getElementById("relationship-strength"),
    strengthValue: document.getElementById("relationship-strength-value"),
  };
  const computeSummary = document.getElementById("compute-summary");
  const tensorOverview = document.getElementById("tensor-overview");
  const tensorAccesses = document.getElementById("tensor-accesses");
  const tensorMemory = document.getElementById("tensor-memory");
  const skipMemoryGaps = document.getElementById("skip-memory-gaps");
  const memoryVisualMode = document.getElementById("memory-visual-mode");
  const memorySummary = document.getElementById("memory-summary");
  const memoriesOverview = document.getElementById("memories-overview");
  const memoryDetail = document.getElementById("memory-detail");
  const selectedTensorPanel = document.getElementById("selected-tensor");
  const timetableGraph = document.getElementById("timetable-graph");
  const timetableGraphStatus = document.getElementById(
    "timetable-graph-status",
  );
  const timetableGraphOverview = document.getElementById(
    "timetable-graph-overview",
  );
  const timetableGraphControls = {
    computeLanes: document.getElementById("timetable-graph-compute-lanes"),
    edgesSelection: document.getElementById("timetable-graph-edges-selection"),
    edgesAll: document.getElementById("timetable-graph-edges-all"),
    groupSize: document.getElementById("timetable-graph-group-size"),
    groupRows: document.getElementById("timetable-graph-group-rows"),
    tensorScale: document.getElementById("timetable-graph-tensor-scale"),
    computeScale: document.getElementById("timetable-graph-compute-scale"),
    groupSizeLegend: document.getElementById(
      "timetable-graph-group-size-legend",
    ),
    tensorColour: document.getElementById("timetable-graph-tensor-colour"),
    tensorLegend: document.getElementById("timetable-graph-tensor-legend"),
    zoomOut: document.getElementById("timetable-graph-zoom-out"),
    zoomIn: document.getElementById("timetable-graph-zoom-in"),
    fit: document.getElementById("timetable-graph-fit"),
    reset: document.getElementById("timetable-graph-reset"),
    expandVisible: document.getElementById("timetable-graph-expand-visible"),
    expandAll: document.getElementById("timetable-graph-expand-all"),
    collapse: document.getElementById("timetable-graph-collapse"),
  };
  const selectedNodePanel = document.getElementById("selected-node");
  const state = {
    hoveredEntity: null,
    activeEntity: null,
    selectedPe: data.pes.find((pe) => pe.total_nodes > 0) || data.pes[0],
    selectedTensor: data.tensors?.[0] || null,
    selectedNode: data.tensors?.[0]
      ? { kind: "tensor", id: data.tensors[0].id }
      : null,
    selectedTensorAccessKey: null,
    selectedLayerName: data.layers?.[0]?.name || null,
    selectedMemoryName: data.memory?.platform_memories?.[0]?.name || null,
    renderedTensorMemoryKey: null,
    renderedMemorySummaryKey: null,
    renderedMemoriesOverviewKey: null,
    renderedMemoryDetailKey: null,
  };
  const pesByName = new Map(data.pes.map((pe) => [pe.name, pe]));
  const tensorsById = new Map(
    (data.tensors || []).map((tensor) => [tensor.id, tensor]),
  );
  const computeNodes = data.compute_nodes || [];
  const memoryLayoutCache = new Map();
  const tensorOverviewMetricsCache = new Map();
  const memoryMetricsCache = new Map();
  const filterContextCache = new Map();
  const relationshipModelCache = new Map();
  const overviewNavigation = new WeakMap();
  const overviewNavigationKeys = ["ArrowUp", "ArrowDown", "PageUp", "PageDown"];
  const overviewHeaderSelector =
    ".pe-overview-chart-header, .layer-overview-header, .memory-overview-header, .tensor-overview-header";

  function overviewPageStep(root) {
    const table = root.querySelector(".overview-table");
    const row =
      table.querySelector("[data-selection-id].selected") ||
      table.querySelector("[data-selection-id]");
    const headerHeight =
      table.querySelector(overviewHeaderSelector)?.getBoundingClientRect()
        .height || 0;
    const rowHeight = row?.getBoundingClientRect().height || 1;
    return Math.max(
      1,
      Math.floor((table.clientHeight - headerHeight) / Math.max(1, rowHeight)),
    );
  }

  function bindOverviewKeyboard(root, ids, selectedId, select) {
    if (!overviewNavigation.has(root)) {
      root.tabIndex = 0;
      root.setAttribute("aria-keyshortcuts", overviewNavigationKeys.join(" "));
      root.addEventListener("keydown", (event) => {
        if (
          !overviewNavigationKeys.includes(event.key) ||
          event.altKey ||
          event.ctrlKey ||
          event.metaKey ||
          event.shiftKey ||
          event.target.closest(
            "input, select, textarea, [contenteditable], [role=separator]",
          ) ||
          (event.target.closest("button, summary") &&
            !event.target.closest("[data-selection-id]")) ||
          !root.querySelector(".overview-table")
        )
          return;
        event.preventDefault();
        event.stopPropagation();
        const config = overviewNavigation.get(root);
        const index = config.ids.indexOf(config.selectedId());
        const forward = event.key === "ArrowDown" || event.key === "PageDown";
        const step = event.key.startsWith("Page") ? overviewPageStep(root) : 1;
        const next =
          index < 0
            ? forward
              ? 0
              : config.ids.length - 1
            : Math.max(
                0,
                Math.min(
                  config.ids.length - 1,
                  index + (forward ? step : -step),
                ),
              );
        if (next === index || !config.ids.length) return;
        // Keep focus on the stable root while selection may replace the rows.
        root.focus({ preventScroll: true });
        const id = config.ids[next];
        config.select(id);
        requestAnimationFrame(() => {
          if (
            document.activeElement !== root ||
            !root.offsetParent ||
            overviewNavigation.get(root).selectedId() !== id
          )
            return;
          const row = [...root.querySelectorAll("[data-selection-id]")].find(
            (element) => element.dataset.selectionId === id,
          );
          const table = row?.closest(".overview-table");
          if (!table) return;
          row.focus({ preventScroll: true });
          const header = table.querySelector(overviewHeaderSelector);
          table.scrollTop +=
            row.getBoundingClientRect().top -
            table.getBoundingClientRect().top -
            (header?.getBoundingClientRect().height || 0);
        });
      });
    }
    overviewNavigation.set(root, { ids, selectedId, select });
  }

  let hoverClearTimer = null;

  function updateHoveredElements(kind) {
    const hovered = state.hoveredEntity;
    if (!kind) {
      return;
    }
    for (const element of document.querySelectorAll(
      `[data-entity-kind="${CSS.escape(kind)}"]`,
    )) {
      const matches =
        kind === hovered?.kind && element.dataset.entityId === hovered.id;
      element.classList.toggle("entity-hovered", matches);
      element.classList.toggle(
        "entity-hover-dimmed",
        kind === hovered?.kind && !matches,
      );
    }
  }

  function setHoveredEntity(kind, id) {
    const next = kind && id ? { kind, id } : null;
    if (
      state.hoveredEntity?.kind === next?.kind &&
      state.hoveredEntity?.id === next?.id
    ) {
      return;
    }
    const previousKind = state.hoveredEntity?.kind;
    state.hoveredEntity = next;
    updateHoveredElements(previousKind);
    if (next?.kind !== previousKind) {
      updateHoveredElements(next?.kind);
    }
    window.dispatchEvent(
      new CustomEvent("gwr-hover-change", { detail: state.hoveredEntity }),
    );
  }

  function markEntityElement(element, kind, id) {
    element.dataset.entityKind = kind;
    element.dataset.entityId = id;
    element.addEventListener("pointerenter", () => {
      clearTimeout(hoverClearTimer);
      setHoveredEntity(kind, id);
    });
    element.addEventListener("pointerleave", (event) => {
      const next = event.relatedTarget?.closest?.(
        `[data-entity-kind="${CSS.escape(kind)}"][data-entity-id="${CSS.escape(id)}"]`,
      );
      if (next) {
        return;
      }
      hoverClearTimer = setTimeout(() => setHoveredEntity(null, null), 0);
    });
    const matches =
      state.hoveredEntity?.kind === kind && state.hoveredEntity.id === id;
    element.classList.toggle("entity-hovered", matches);
    element.classList.toggle(
      "entity-hover-dimmed",
      state.hoveredEntity?.kind === kind && !matches,
    );
  }

  document.getElementById("source-path").textContent = data.summary.timetable;

  function option(value, label) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = label;
    return option;
  }

  function labelFromName(name) {
    const words = String(name).replaceAll("_", " ");
    return words.charAt(0).toUpperCase() + words.slice(1);
  }

  const machineOpTypes = (
    data.machine_ops?.length
      ? data.machine_ops
      : [
          ...new Set(
            (data.layers || []).flatMap((layer) =>
              Object.keys(layer.machine_ops || {}),
            ),
          ),
        ]
          .filter((name) => name !== "total")
          .map((name) => ({ name, label: labelFromName(name) }))
  )
    .map((op) => ({
      name: op.name,
      label: op.label || labelFromName(op.name),
      colour: op.colour || "var(--activity)",
    }))
    .sort((left, right) => left.label.localeCompare(right.label));
  const machineOpKeys = machineOpTypes.map((op) => op.name);
  const computeNodePalette = [
    "#2f6f9f",
    "#3a8f72",
    "#bf6b3d",
    "#8b6fb3",
    "#a47d22",
    "#4f8f9d",
    "#a65d75",
    "#64748b",
  ];
  const computeNodeTypes = [...(data.ops || [])]
    .sort((left, right) => left.localeCompare(right))
    .map((name, index) => ({
      name,
      label: labelFromName(name),
      colour: computeNodePalette[index % computeNodePalette.length],
    }));

  function emptyMachineOps() {
    return Object.fromEntries([
      ["total", 0n],
      ...machineOpKeys.map((name) => [name, 0n]),
    ]);
  }

  function formatCount(value) {
    if (typeof value === "number") {
      return Math.abs(value) < 10000
        ? fmt.format(value)
        : compactCountFormat.format(value);
    }
    const integer = toBigInt(value);
    if (integer < 10000n) {
      return fmt.format(integer);
    }
    return compactCountFormat.format(integer);
  }

  function escapeHtml(value) {
    return String(value)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#39;");
  }

  function createOverviewColumnConfiguration(definitions, defaultKeys) {
    const definitionsByKey = new Map(
      definitions.map((definition) => [definition.key, definition]),
    );
    const widths = new Map(
      definitions.map((definition) => [
        definition.key,
        definition.defaultWidth || 120,
      ]),
    );
    const initialKeys = defaultKeys.filter((key) => definitionsByKey.has(key));
    let keys = [...initialKeys];

    function visible() {
      return keys.map((key) => ({
        ...definitionsByKey.get(key),
        width: widths.get(key),
      }));
    }

    function add(key) {
      if (!definitionsByKey.has(key) || keys.includes(key)) {
        return false;
      }
      keys.push(key);
      return true;
    }

    function remove(key) {
      if (keys.length <= 1 || !keys.includes(key)) {
        return false;
      }
      keys = keys.filter((candidate) => candidate !== key);
      return true;
    }

    function setWidth(key, width) {
      const definition = definitionsByKey.get(key);
      if (!definition) {
        return;
      }
      const minimum = definition.minWidth || 80;
      widths.set(
        key,
        Math.min(Math.max(Number(width) || minimum, minimum), 1600),
      );
    }

    function apply(keysToShow) {
      const valid = keysToShow.filter((key) => definitionsByKey.has(key));
      keys = [...new Set(valid)];
    }

    function clear() {
      keys = [];
    }

    function equalize() {
      for (const key of keys) {
        widths.set(key, 100);
      }
    }

    function setVisible(key, visible) {
      return visible ? add(key) : remove(key);
    }

    function snapshot() {
      return visible().map(({ key, width }) => ({ key, width }));
    }

    function restore(columns) {
      if (!Array.isArray(columns)) {
        return;
      }
      apply(columns.map((column) => column.key));
      for (const column of columns) {
        setWidth(column.key, column.width);
      }
    }

    function reset() {
      keys = [...initialKeys];
      for (const definition of definitions) {
        widths.set(definition.key, definition.defaultWidth || 120);
      }
    }

    return {
      definitions,
      visible,
      add,
      remove,
      setWidth,
      apply,
      clear,
      equalize,
      setVisible,
      snapshot,
      restore,
      reset,
    };
  }

  function matchingPickerValues(picker, labelFor = (value) => value) {
    let expression;
    try {
      expression = picker.input.value
        ? new RegExp(picker.input.value, "i")
        : null;
      picker.input.removeAttribute("aria-invalid");
    } catch {
      picker.input.setAttribute("aria-invalid", "true");
      picker.status.textContent = "Invalid regular expression";
      return null;
    }
    const matches = expression
      ? picker.values.filter((value) => expression.test(labelFor(value)))
      : picker.values;
    const matching = new Set(matches);
    for (const option of picker.container.querySelectorAll(".filter-option")) {
      option.hidden = !matching.has(option.querySelector("input").value);
    }
    picker.status.textContent = `${fmt.format(matches.length)} shown`;
    return matches;
  }

  const columnPickerPatterns = new WeakMap();

  function overviewColumnControlsMarkup(configuration, presets = []) {
    const visible = configuration.visible();
    const visibleKeys = new Set(visible.map((column) => column.key));
    return `
      <div class="overview-column-controls">
        <details class="overview-column-picker filter-picker">
          <summary><span>Columns</span><strong>${visible.length} of ${configuration.definitions.length}</strong></summary>
          <div class="filter-picker-actions">
            ${presets.map((preset) => `<button type="button" data-column-preset="${escapeHtml(preset.key)}"${preset.keys.length ? "" : " disabled"}>${escapeHtml(preset.label)}</button>`).join("")}
            <button type="button" data-column-all${visible.length === configuration.definitions.length ? " disabled" : ""}>All</button>
            <button type="button" data-column-none${visible.length ? "" : " disabled"}>None</button>
          </div>
          <div class="filter-pattern">
            <label>Regular expression
              <input data-column-pattern type="text" value="${escapeHtml(columnPickerPatterns.get(configuration) || "")}" placeholder="Compute nodes:|Machine ops:" spellcheck="false">
            </label>
            <div>
              <button type="button" data-column-select-matches>Select matches</button>
              <button type="button" data-column-clear-pattern>Clear</button>
            </div>
            <output data-column-pattern-status aria-live="polite"></output>
          </div>
          <div class="filter-options overview-column-options" role="group" aria-label="Visible columns">
            ${configuration.definitions
              .map((column) => {
                const selected = visibleKeys.has(column.key);
                return `<label class="filter-option"><input type="checkbox" value="${escapeHtml(column.key)}" data-column-toggle="${escapeHtml(column.key)}"${selected ? " checked" : ""}${selected && visible.length <= 1 ? " disabled" : ""}><span>${escapeHtml(column.label)}</span></label>`;
              })
              .join("")}
          </div>
        </details>
      </div>`;
  }

  function bindOverviewColumnControls(
    container,
    configuration,
    presets,
    changed,
  ) {
    const picker = container.querySelector(".overview-column-picker");
    function applyChange() {
      const wasOpen = picker?.open;
      changed();
      if (wasOpen) {
        container.querySelector(".overview-column-picker").open = true;
      }
    }
    const input = picker.querySelector("[data-column-pattern]");
    const labels = new Map(
      configuration.definitions.map((column) => [column.key, column.label]),
    );
    const matches = () =>
      matchingPickerValues(
        {
          input,
          status: picker.querySelector("[data-column-pattern-status]"),
          container: picker.querySelector(".overview-column-options"),
          values: [...labels.keys()],
        },
        (key) => labels.get(key),
      );
    input.addEventListener("input", () => {
      columnPickerPatterns.set(configuration, input.value);
      matches();
    });
    picker
      .querySelector("[data-column-select-matches]")
      .addEventListener("click", () => {
        const keys = matches();
        if (keys === null) return;
        configuration.apply(keys);
        applyChange();
      });
    picker
      .querySelector("[data-column-clear-pattern]")
      .addEventListener("click", () => {
        input.value = "";
        columnPickerPatterns.delete(configuration);
        matches();
        input.focus();
      });
    matches();
    for (const input of container.querySelectorAll("[data-column-toggle]")) {
      input.addEventListener("change", () => {
        if (
          configuration.setVisible(input.dataset.columnToggle, input.checked)
        ) {
          applyChange();
        } else {
          input.checked = !input.checked;
        }
      });
    }
    const presetsByKey = new Map(presets.map((preset) => [preset.key, preset]));
    for (const button of container.querySelectorAll("[data-column-preset]")) {
      button.addEventListener("click", () => {
        configuration.apply(
          presetsByKey.get(button.dataset.columnPreset)?.keys || [],
        );
        applyChange();
      });
    }
    container
      .querySelector("[data-column-all]")
      ?.addEventListener("click", () => {
        configuration.apply(
          configuration.definitions.map((definition) => definition.key),
        );
        applyChange();
      });
    container
      .querySelector("[data-column-none]")
      ?.addEventListener("click", () => {
        configuration.clear();
        applyChange();
      });
  }

  function overviewGridTemplate(configuration, identityWidth) {
    return `${identityWidth}px ${configuration
      .visible()
      .map(
        (column) =>
          `minmax(${column.minWidth || 80}px, ${Math.max(column.width, 1)}fr)`,
      )
      .join(" ")}`;
  }

  function overviewTableMinimumWidth(configuration, identityWidth) {
    return configuration
      .visible()
      .reduce(
        (total, column) => total + (column.minWidth || 80),
        identityWidth,
      );
  }

  function bindOverviewColumnResizing(
    container,
    configuration,
    identityWidth,
    changed,
  ) {
    const table = container.querySelector(".overview-table");

    function normalizeWidths() {
      for (const header of container.querySelectorAll(
        ".overview-column-header[data-column-key]",
      )) {
        configuration.setWidth(
          header.dataset.columnKey,
          header.getBoundingClientRect().width,
        );
      }
    }

    function preview() {
      table.style.setProperty(
        "--overview-grid-template",
        overviewGridTemplate(configuration, identityWidth),
      );
    }

    for (const handle of container.querySelectorAll("[data-column-resize]")) {
      handle.addEventListener("pointerdown", (event) => {
        event.preventDefault();
        event.stopPropagation();
        normalizeWidths();
        const startX = event.clientX;
        const header = handle.closest(".overview-column-header");
        const startWidth = header.getBoundingClientRect().width;
        const pointerId = event.pointerId;
        let moved = false;
        handle.setPointerCapture(pointerId);
        handle.classList.add("resizing");

        const move = (moveEvent) => {
          moved ||= moveEvent.clientX !== startX;
          configuration.setWidth(
            handle.dataset.columnResize,
            startWidth + moveEvent.clientX - startX,
          );
          preview();
        };
        const finish = () => {
          handle.removeEventListener("pointermove", move);
          handle.classList.remove("resizing");
          if (handle.hasPointerCapture(pointerId)) {
            handle.releasePointerCapture(pointerId);
          }
          if (moved) {
            changed();
          }
        };
        handle.addEventListener("pointermove", move);
        handle.addEventListener("pointerup", finish, { once: true });
        handle.addEventListener("pointercancel", finish, { once: true });
      });
      handle.addEventListener("dblclick", (event) => {
        event.preventDefault();
        event.stopPropagation();
        configuration.equalize();
        changed();
      });
      handle.addEventListener("keydown", (event) => {
        if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") {
          return;
        }
        event.preventDefault();
        event.stopPropagation();
        normalizeWidths();
        const header = handle.closest(".overview-column-header");
        const delta = event.key === "ArrowLeft" ? -10 : 10;
        configuration.setWidth(
          handle.dataset.columnResize,
          header.getBoundingClientRect().width + delta,
        );
        changed();
      });
    }
  }

  function metricActionMarkup(content, measure, title, className) {
    if (!measure) {
      return `<div class="${className}">${content}</div>`;
    }
    const selected =
      document.getElementById("pe-overview-measure")?.value === measure;
    return `
      <button type="button" class="${className} metric-action" data-pe-overview-measure="${escapeHtml(measure)}" aria-pressed="${selected}" title="${escapeHtml(title)}">
        ${content}
      </button>`;
  }

  function metricStripMarkup({
    label,
    total,
    entries,
    formatter = formatCount,
    maximum = total,
    marker,
    measure,
    caption = "",
    showZero = false,
  }) {
    const totalValue = toBigInt(total);
    const scaleMaximum = bigIntMax(toBigInt(maximum), 1n);
    const visibleEntries = entries.filter(
      (entry) => showZero || toBigInt(entry.value) > 0n,
    );
    const ariaBreakdown = visibleEntries
      .map(
        (entry) =>
          `${entry.formatter?.(entry.value) || formatter(entry.value)} ${entry.label}`,
      )
      .join(", ");
    const heading = `<span>${escapeHtml(label)}</span><strong>${formatter(totalValue)}</strong>`;
    return `
      <section class="metric-strip" aria-label="${escapeHtml(`${label} ${formatter(totalValue)}${ariaBreakdown ? `: ${ariaBreakdown}` : ""}`)}">
        ${metricActionMarkup(heading, measure, `Show ${label} in PE overview`, "metric-strip-heading")}
        <div class="metric-strip-track" aria-hidden="true">
          ${entries
            .filter((entry) => toBigInt(entry.value) > 0n)
            .map((entry) => {
              const value = toBigInt(entry.value);
              const formatted = entry.formatter?.(value) || formatter(value);
              const percent = ratioPercent(value, bigIntMax(totalValue, 1n));
              const title = `${entry.label}: ${formatted} (${percent.toFixed(1)}% of ${label})`;
              return `<i title="${escapeHtml(title)}" style="--metric-colour: ${escapeHtml(entry.colour || "var(--activity)")}; width: ${Math.min(ratioPercent(value, scaleMaximum), 100)}%"></i>`;
            })
            .join("")}
          ${marker === undefined ? "" : `<b style="left: ${Math.min(Math.max(Number(marker || 0), 0), 100)}%"></b>`}
        </div>
        <div class="metric-strip-legend">
          ${visibleEntries
            .map((entry) => {
              const value = toBigInt(entry.value);
              const formatted = entry.formatter?.(value) || formatter(value);
              const percent = ratioPercent(value, bigIntMax(totalValue, 1n));
              const content = `<i style="--metric-colour: ${escapeHtml(entry.colour || "var(--activity)")}"></i><span>${escapeHtml(entry.label)}</span><strong>${formatted}</strong>`;
              return metricActionMarkup(
                content,
                entry.measure,
                `Show ${entry.label} in PE overview; ${formatted}, ${percent.toFixed(1)}% of ${label}`,
                "metric-strip-key",
              );
            })
            .join("")}
        </div>
        ${caption ? `<p class="metric-strip-caption">${escapeHtml(caption)}</p>` : ""}
      </section>`;
  }

  function machineOpsMarkup(ops = {}, options = {}) {
    const entries = machineOpTypes.map((op) => ({
      label: op.label,
      value: toBigInt(ops[op.name]),
      colour: op.colour,
      measure: `compute:machine-op:${op.name}`,
    }));
    const total = toBigInt(
      ops.total ?? entries.reduce((sum, entry) => sum + entry.value, 0n),
    );
    return metricStripMarkup({
      label: "Machine ops",
      total,
      entries,
      measure: "compute:machine-ops",
      showZero: true,
      ...options,
    });
  }

  function computeNodesMarkup(total, byOp = {}, options = {}) {
    const entries = computeNodeTypes.map((op) => ({
      label: op.label,
      value: Number(byOp[op.name] || 0),
      colour: op.colour,
    }));
    return metricStripMarkup({
      label: "Compute nodes",
      total: Number(total || 0),
      entries,
      measure: "compute:compute-nodes",
      showZero: true,
      ...options,
    });
  }

  function trafficMarkup({
    read,
    write,
    maximum = bigIntMax(toBigInt(read), toBigInt(write)),
    averageRead,
    averageWrite,
    caption = "",
  }) {
    const scaleMaximum = bigIntMax(toBigInt(maximum), 1n);
    const rows = [
      {
        label: "Read",
        value: toBigInt(read),
        colour: "var(--read)",
        measure: "data:read",
        average: averageRead,
      },
      {
        label: "Written",
        value: toBigInt(write),
        colour: "var(--write)",
        measure: "data:write",
        average: averageWrite,
      },
    ];
    return `
      <section class="metric-traffic" aria-label="Read ${formatBytes(read)}, Written ${formatBytes(write)}">
        ${rows
          .map((row) => {
            const formatted = formatBytes(row.value);
            const content = `<span>${row.label}</span><div class="metric-traffic-track" style="--metric-colour: ${row.colour}"><i style="width: ${ratioPercent(row.value, scaleMaximum)}%"></i>${row.average === undefined ? "" : `<b style="left: ${ratioPercent(row.average, scaleMaximum)}%"></b>`}</div><strong>${formatted}</strong>`;
            return metricActionMarkup(
              content,
              row.measure,
              `Show ${row.label} in PE overview; ${formatted}`,
              "metric-traffic-row",
            );
          })
          .join("")}
        ${caption ? `<p class="metric-strip-caption">${escapeHtml(caption)}</p>` : ""}
      </section>`;
  }
  function dims() {
    if (data.platform) {
      const platformRows = Number(data.platform.rows || 0);
      const platformCols = Number(data.platform.cols || 0);
      const fabricRows = (data.platform.fabrics || []).map((fabric) =>
        Number(fabric.rows || 0),
      );
      const fabricCols = (data.platform.fabrics || []).map((fabric) =>
        Number(fabric.cols || 0),
      );
      const peRows = data.pes.map((pe) => Number(pe.row) + 1);
      const peCols = data.pes.map((pe) => Number(pe.col) + 1);
      return [
        Math.max(platformRows, ...fabricRows, ...peRows, 1),
        Math.max(platformCols, ...fabricCols, ...peCols, 1),
      ];
    }

    const peRows = data.pes.map((pe) => Number(pe.row) + 1);
    const peCols = data.pes.map((pe) => Number(pe.col) + 1);
    return [Math.max(...peRows, 1), Math.max(...peCols, 1)];
  }

  function toBigInt(value, fallback = 0n) {
    if (value === undefined || value === null || value === "") {
      return fallback;
    }
    return BigInt(value);
  }

  function bigIntMax(left, right) {
    return left > right ? left : right;
  }

  function bigIntMin(left, right) {
    return left < right ? left : right;
  }

  function bigIntCompare(left, right) {
    const leftValue = toBigInt(left);
    const rightValue = toBigInt(right);
    return leftValue < rightValue ? -1 : leftValue > rightValue ? 1 : 0;
  }

  function bigIntToNumber(value) {
    return Number(value);
  }

  function ratioPercent(value, maximum) {
    const denominator = Number(maximum);
    if (denominator === 0) {
      return 0;
    }
    return (Number(value) / denominator) * 100;
  }

  function integerAverage(total, count) {
    return count ? Number(total) / count : 0;
  }

  function scaleInteger(value, numerator, denominator) {
    const divisor = toBigInt(denominator);
    if (divisor === 0n) {
      return 0n;
    }
    return (toBigInt(value) * toBigInt(numerator)) / divisor;
  }

  function addressRange(start, bytes) {
    const rangeStart = toBigInt(start);
    return [rangeStart, rangeStart + toBigInt(bytes)];
  }

  function overlapBytes(leftStart, leftBytes, rightStart, rightBytes) {
    const [leftRangeStart, leftRangeEnd] = addressRange(leftStart, leftBytes);
    const [rightRangeStart, rightRangeEnd] = addressRange(
      rightStart,
      rightBytes,
    );
    return bigIntMax(
      bigIntMin(leftRangeEnd, rightRangeEnd) -
        bigIntMax(leftRangeStart, rightRangeStart),
      0n,
    );
  }

  function clipTensorToMemory(tensor, memory) {
    const [tensorStart, tensorEnd] = addressRange(
      tensor.addr,
      bigIntMax(toBigInt(tensor.num_bytes), 1n),
    );
    const [memoryStart, memoryEnd] = addressRange(
      memory.base_addr,
      memory.capacity_bytes,
    );
    const start = bigIntMax(tensorStart, memoryStart);
    const end = bigIntMin(tensorEnd, memoryEnd);
    if (end <= start) {
      return null;
    }
    return {
      id: tensor.id,
      addr: start.toString(),
      num_bytes: (end - start).toString(),
      tensor,
    };
  }

  function formatHex(value) {
    return `0x${toBigInt(value).toString(16)}`;
  }

  function formatBytes(bytes) {
    const units = ["B", "KiB", "MiB", "GiB"];
    if (typeof bytes === "number" && !Number.isInteger(bytes)) {
      let value = bytes;
      let unit = units[0];
      for (let index = 0; index < units.length - 1 && value >= 1024; index++) {
        value /= 1024;
        unit = units[index + 1];
      }
      const precision = value >= 10 || unit === "B" ? 0 : 1;
      return `${value.toFixed(precision)} ${unit}`;
    }
    const value = toBigInt(bytes);
    let divisor = 1n;
    let unitIndex = 0;
    while (unitIndex < units.length - 1 && value >= divisor * 1024n) {
      divisor *= 1024n;
      unitIndex += 1;
    }
    if (unitIndex === 0) {
      return `${fmt.format(value)} ${units[unitIndex]}`;
    }
    const whole = value / divisor;
    if (whole >= 10n) {
      const rounded = (value + divisor / 2n) / divisor;
      return `${fmt.format(rounded)} ${units[unitIndex]}`;
    }
    const roundedTenths = (value * 10n + divisor / 2n) / divisor;
    return `${fmt.format(roundedTenths / 10n)}.${roundedTenths % 10n} ${units[unitIndex]}`;
  }

  Object.assign(App, {
    matchingPickerValues,
    bindOverviewKeyboard,
    overviewNavigationKeys,
    data,
    fmt,
    controls,
    viewControls,
    grid,
    rowAxis,
    colAxis,
    selectedPanel,
    timetableSummary,
    layerSummary,
    layerDetail,
    relationshipBundle,
    relationshipControls,
    computeSummary,
    tensorOverview,
    tensorAccesses,
    tensorMemory,
    skipMemoryGaps,
    memoryVisualMode,
    memorySummary,
    memoriesOverview,
    memoryDetail,
    selectedTensorPanel,
    timetableGraph,
    timetableGraphStatus,
    timetableGraphOverview,
    timetableGraphControls,
    selectedNodePanel,
    state,
    pesByName,
    tensorsById,
    computeNodes,
    memoryLayoutCache,
    tensorOverviewMetricsCache,
    memoryMetricsCache,
    filterContextCache,
    relationshipModelCache,
    markEntityElement,
    setHoveredEntity,
    option,
    labelFromName,
    machineOpTypes,
    machineOpKeys,
    computeNodeTypes,
    emptyMachineOps,
    formatCount,
    escapeHtml,
    createOverviewColumnConfiguration,
    overviewColumnControlsMarkup,
    bindOverviewColumnControls,
    overviewGridTemplate,
    overviewTableMinimumWidth,
    bindOverviewColumnResizing,
    metricStripMarkup,
    machineOpsMarkup,
    computeNodesMarkup,
    trafficMarkup,
    dims,
    toBigInt,
    bigIntMax,
    bigIntCompare,
    bigIntToNumber,
    ratioPercent,
    integerAverage,
    scaleInteger,
    addressRange,
    overlapBytes,
    clipTensorToMemory,
    formatHex,
    formatBytes,
  });
})();
