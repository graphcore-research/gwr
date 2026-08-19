// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

import { browserAdapters } from "./lib/adapters.mjs";
import { readConfig } from "./lib/config.mjs";
import { prepareReports } from "./lib/reports.mjs";

const presets = {
  summary: ["timetable-summary", "compute-summary", "memory-summary"],
  layers: [
    "timetable-summary",
    "layer-summary",
    "layer-details",
    "relationships",
  ],
  compute: ["compute-summary", "pe-grid", "relationships", "selected-pe"],
  memory: [
    "memory-summary",
    "memories-overview",
    "relationships",
    "pe-grid",
    "memory-details",
    "tensor-memory",
    "selected-tensor",
  ],
  tensor: ["tensor-memory", "selected-tensor", "pe-grid"],
};

const config = readConfig(process.argv.slice(2));
const reports = await prepareReports(config);
const results = [];
const failures = [];

await mkdir(config.output, { recursive: true });

try {
  for (const adapter of browserAdapters(config)) {
    try {
      const url = await reports.sample();
      assert.match(url, /^file:/, "browser test must open a file URL");
      const screenshot = path.join(config.output, `${adapter.name}.png`);
      const evidence = await adapter.withSession(url, (session) =>
        exerciseReport(session, screenshot),
      );
      const startupError = await exerciseStartupError(adapter);
      results.push({
        browser: adapter.name,
        browser_version: adapter.version,
        report_url: url,
        screenshot,
        evidence,
        startup_error: startupError,
      });
      process.stdout.write(`${adapter.name} browser checks complete\n`);
    } catch (error) {
      failures.push(`${adapter.name}: ${error.stack || error.message}`);
    }
  }
} finally {
  await reports.close();
}

await writeFile(
  path.join(config.output, "browser-test.json"),
  `${JSON.stringify({ results, failures }, null, 2)}\n`,
);
if (failures.length) {
  throw new Error(`Browser checks failed:\n${failures.join("\n")}`);
}

async function exerciseReport(session, screenshot) {
  await click(session, "#workspace-reset");
  try {
    const initial = await initialState(session);
    assert.equal(initial.ready, "complete");
    assert.equal(initial.summary_ready, "complete");
    assert.ok(initial.source, "source path is empty");
    assert.equal(initial.workspace_bars, 12);
    assert.ok(
      initial.stats.every(Boolean),
      "one or more summary statistics are empty",
    );
    assert.deepStrictEqual(initial.visible, presets.summary);
    await assertActivePreset(session, "summary");

    await selectPreset(session, "layers");
    const layers = await visiblePanels(session);
    assert.deepStrictEqual(layers, presets.layers);
    const selection = await exerciseLayerSelection(session);
    const regexFilter = await exerciseRegexFilter(session, selection.id);

    await selectPreset(session, "compute");
    const compute = await visiblePanels(session);
    assert.deepStrictEqual(compute, presets.compute);
    const peModes = await exercisePeModes(session);

    await selectPreset(session, "layers");
    const relationships = await exerciseRelationships(session);

    await selectPreset(session, "memory");
    assert.deepStrictEqual(await visiblePanels(session), presets.memory);
    const memory = await contentState(session, [
      "#memory-summary",
      "#memories-overview",
      "#memory-detail",
      "#tensor-memory",
    ]);
    assert.ok(
      Object.values(memory).every(Boolean),
      `memory panels did not all render: ${JSON.stringify(memory)}`,
    );

    await selectPreset(session, "tensor");
    assert.deepStrictEqual(await visiblePanels(session), presets.tensor);
    const tensor = await tensorState(session);
    assert.equal(tensor.tensor_memory_rendered, true);
    assert.equal(tensor.tensor_detail_rendered, true);
    assert.equal(tensor.pe_grid_visible, true);
    assert.equal(tensor.pe_measure, "tensor:read");

    const workspace = await exerciseWorkspace(session);
    await session.screenshot(screenshot);

    return {
      initial,
      presets: { layers, compute },
      selection,
      regex_filter: regexFilter,
      pe_modes: peModes,
      relationships,
      memory,
      tensor,
      workspace,
    };
  } finally {
    await click(session, "#workspace-reset");
  }
}

async function exerciseLayerSelection(session) {
  const target = await session.evaluate(
    `(() => {
      const row = document.querySelector(".layer-summary-row");
      if (!row) throw new Error("No layer summary row was rendered");
      row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      return row.dataset.selectId;
    })()`,
  );
  await session.wait(260);
  const selected = await session.evaluate(
    `(() => {
      const row = document.querySelector(".layer-summary-row.selected");
      return {
        id: row?.dataset.selectId || null,
        pressed: row?.getAttribute("aria-pressed"),
      };
    })()`,
  );
  assert.equal(selected.id, target);
  assert.equal(selected.pressed, "true");

  await session.evaluate(
    `(() => {
      const row = document.querySelector(".layer-summary-row.selected");
      if (!row) throw new Error("Selected layer row disappeared");
      row.dispatchEvent(new MouseEvent("dblclick", {
        bubbles: true,
        cancelable: true,
      }));
    })()`,
  );
  await session.wait(50);
  assert.equal(await text(session, "#layer-filter-summary"), target);
  return selected;
}

async function exerciseRegexFilter(session, layer) {
  await click(session, "#layer-filter-all");
  const pattern = `^${escapeRegularExpression(layer)}$`;
  await session.evaluate(
    `(() => {
      const details = document.getElementById("layer-filter").closest("details");
      details.open = true;
      details.dispatchEvent(new Event("toggle"));
      const input = document.getElementById("layer-filter-pattern");
      input.value = ${JSON.stringify(pattern)};
      input.dispatchEvent(new Event("input", { bubbles: true }));
      document.getElementById("layer-filter-select-matches").click();
    })()`,
  );
  await session.wait(50);
  const result = {
    summary: await text(session, "#layer-filter-summary"),
    status: await text(session, "#layer-filter-pattern-status"),
  };
  assert.equal(result.summary, layer);
  assert.match(result.status, /\b1 shown\b/);
  await click(session, "#layer-filter-all");
  return result;
}

async function exercisePeModes(session) {
  const initial = await peModeState(session);
  assert.deepStrictEqual(initial, {
    chart_visible: false,
    grid_visible: true,
    chart_pressed: "false",
    grid_pressed: "true",
  });

  await click(session, "[data-pe-overview-mode='chart']");
  const chart = await peModeState(session);
  assert.deepStrictEqual(chart, {
    chart_visible: true,
    grid_visible: false,
    chart_pressed: "true",
    grid_pressed: "false",
  });

  await click(session, "[data-pe-overview-mode='grid']");
  const grid = await peModeState(session);
  assert.deepStrictEqual(grid, initial);
  return { initial, chart, grid };
}

async function exerciseRelationships(session) {
  await session.evaluate(
    `(() => {
      const select = document.getElementById("relationship-mode");
      select.value = "tensor-pe";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    })()`,
  );
  await session.wait(50);
  const state = await session.evaluate(
    `(() => {
      const bundle = document.getElementById("relationship-bundle");
      return {
        mode: document.getElementById("relationship-mode").value,
        rendered: Boolean(bundle.firstElementChild),
        description: bundle.textContent.replace(/\\s+/g, " ").trim(),
        status: [...bundle.querySelectorAll(".relationship-status > *")].map(
          (element) => element.textContent.replace(/\\s+/g, " ").trim(),
        ),
      };
    })()`,
  );
  assert.equal(state.mode, "tensor-pe");
  assert.equal(state.rendered, true);
  assert.ok(state.description, "relationship output is empty");
  return state;
}

async function exerciseWorkspace(session) {
  await session.evaluate(
    `(() => {
      const layout = document.getElementById("view-layout");
      layout.value = "two";
      layout.dispatchEvent(new Event("change", { bubbles: true }));
      const panel = document.querySelector("[data-view]:not([hidden])");
      const width = panel.querySelector(".workspace-panel-width");
      width.value = "2";
      width.dispatchEvent(new Event("change", { bubbles: true }));
      panel.querySelector('[data-workspace-action="down"]').click();
      panel.querySelector(".workspace-collapse").click();
      panel.querySelector(".workspace-focus").click();
    })()`,
  );
  const focused = await session.evaluate(
    `(() => {
      const views = document.getElementById("views");
      const panel = views.querySelector(".workspace-focused");
      return {
        container: views.classList.contains("workspace-has-focus"),
        panel: panel?.dataset.view || null,
        pressed:
          panel?.querySelector(".workspace-focus").getAttribute("aria-pressed") ||
          null,
      };
    })()`,
  );
  assert.deepStrictEqual(focused, {
    container: true,
    panel: "tensor-memory",
    pressed: "true",
  });
  await click(session, '[data-view="tensor-memory"] .workspace-focus');

  const beforeReload = await workspaceState(session);
  await session.reload();
  const afterReload = await workspaceState(session);
  assert.deepStrictEqual(afterReload, beforeReload);
  return { focused, before_reload: beforeReload, after_reload: afterReload };
}

async function exerciseStartupError(adapter) {
  const url = await reports.brokenSample();
  const state = await adapter.withSession(
    url,
    (session) =>
      session.evaluate(
        `(() => ({
          error: document.documentElement.dataset.gwrError || "",
          warning: document.getElementById("warnings").textContent.trim(),
        }))()`,
      ),
    { allowStartupError: true },
  );
  assert.match(state.error, /Report payload is missing/);
  assert.match(state.warning, /Unable to start visualisation/);
  assert.match(state.warning, /Report payload is missing/);
  return state;
}

async function selectPreset(session, name) {
  await click(session, `[data-preset='${name}']`);
  await assertActivePreset(session, name);
}

async function assertActivePreset(session, name) {
  const pressed = await session.evaluate(
    `(() => Object.fromEntries(
      [...document.querySelectorAll("[data-preset]")].map(
        (button) => [
          button.dataset.preset,
          button.getAttribute("aria-pressed"),
        ],
      ),
    ))()`,
  );
  for (const [preset, value] of Object.entries(pressed)) {
    assert.equal(value, preset === name ? "true" : "false");
  }
}

async function initialState(session) {
  return session.evaluate(
    `(() => ({
      ready: document.documentElement.dataset.gwrReady,
      summary_ready: document.documentElement.dataset.gwrSummaryReady,
      stats: [...document.querySelectorAll(".stats strong, .stats em")].map(
        (element) => element.textContent.trim(),
      ),
      source: document.getElementById("source-path").textContent.trim(),
      visible: [...document.querySelectorAll("[data-view]:not([hidden])")].map(
        (element) => element.dataset.view,
      ),
      workspace_bars: document.querySelectorAll(".workspace-panel-bar").length,
    }))()`,
  );
}

async function peModeState(session) {
  return session.evaluate(
    `(() => ({
      chart_visible: !document.getElementById("pe-overview-chart").hidden,
      grid_visible: !document.getElementById("pe-overview-grid").hidden,
      chart_pressed: document
        .querySelector('[data-pe-overview-mode="chart"]')
        .getAttribute("aria-pressed"),
      grid_pressed: document
        .querySelector('[data-pe-overview-mode="grid"]')
        .getAttribute("aria-pressed"),
    }))()`,
  );
}

async function workspaceState(session) {
  return session.evaluate(
    `(() => {
      const panel = document.querySelector('[data-view="tensor-memory"]');
      return {
        layout: document.getElementById("view-layout").value,
        visible_order: [
          ...document.querySelectorAll("[data-view]:not([hidden])"),
        ].map((element) => element.dataset.view),
        panel: {
          width: panel.dataset.workspaceWidth,
          collapsed: panel.classList.contains("workspace-collapsed"),
          collapse_expanded: panel
            .querySelector(".workspace-collapse")
            .getAttribute("aria-expanded"),
        },
      };
    })()`,
  );
}

async function click(session, selector) {
  await session.evaluate(
    `(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (!element) {
        throw new Error(${JSON.stringify(`Missing ${selector}`)});
      }
      element.click();
    })()`,
  );
  await session.wait(50);
}

async function visiblePanels(session) {
  return session.evaluate(
    `(() => [...document.querySelectorAll("[data-view]:not([hidden])")].map(
      (element) => element.dataset.view,
    ))()`,
  );
}

async function contentState(session, selectors) {
  return session.evaluate(
    `(() => Object.fromEntries(
      ${JSON.stringify(selectors)}.map((selector) => {
        const element = document.querySelector(selector);
        return [selector, Boolean(element?.textContent.trim())];
      }),
    ))()`,
  );
}

async function tensorState(session) {
  return session.evaluate(
    `(() => ({
      tensor_memory_rendered: Boolean(
        document.getElementById("tensor-memory").textContent.trim(),
      ),
      tensor_detail_rendered: Boolean(
        document.getElementById("selected-tensor").textContent.trim(),
      ),
      pe_grid_visible: !document.getElementById("pe-overview-grid").hidden,
      pe_measure: document.getElementById("pe-overview-measure").value,
    }))()`,
  );
}

async function text(session, selector) {
  return session.evaluate(
    `(() => document.querySelector(${JSON.stringify(selector)})?.textContent.trim() || "")()`,
  );
}

function escapeRegularExpression(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
