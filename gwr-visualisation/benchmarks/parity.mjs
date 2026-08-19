// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";

import pixelmatch from "pixelmatch";
import { PNG } from "pngjs";

import { browserAdapters } from "./lib/adapters.mjs";
import { readConfig } from "./lib/config.mjs";
import { prepareReports } from "./lib/reports.mjs";

const config = readConfig(process.argv.slice(2));
const reports = await prepareReports(config);
const adapters = browserAdapters(config);
const results = [];
const failures = [];

await mkdir(config.output, { recursive: true });

try {
  for (const adapter of adapters) {
    const snapshots = {};
    const screenshots = {};
    for (const implementation of ["javascript", "wasm"]) {
      const url = await reports.sample(implementation);
      const screenshot = path.join(config.output, `${adapter.name}-${implementation}.png`);
      snapshots[implementation] = await adapter.withSession(url, (session) =>
        exerciseReport(session, screenshot),
      );
      screenshots[implementation] = screenshot;
    }
    try {
      assert.deepStrictEqual(snapshots.wasm, snapshots.javascript);
    } catch (error) {
      failures.push(`${adapter.name} semantic parity: ${error.message}`);
    }
    const screenshot = await compareScreenshots(
      screenshots.javascript,
      screenshots.wasm,
      path.join(config.output, `${adapter.name}-diff.png`),
    );
    if (screenshot.difference_ratio > 0.05) {
      failures.push(
        `${adapter.name} screenshot difference was ${(screenshot.difference_ratio * 100).toFixed(2)}%; limit is 5%`,
      );
    }
    results.push({ browser: adapter.name, snapshots, screenshot });
    process.stdout.write(`${adapter.name} parity complete\n`);
  }
  await writeFile(
    path.join(config.output, "parity.json"),
    `${JSON.stringify({ results, failures }, null, 2)}\n`,
  );
  if (failures.length) {
    throw new Error(`Browser parity failed:\n${failures.join("\n")}`);
  }
} finally {
  await reports.close();
}

async function exerciseReport(session, screenshot) {
  const initial = await snapshot(session);
  await click(session, "[data-preset='layers']");
  const layers = await visiblePanels(session);
  await session.evaluate(
    `(() => document.querySelector('.layer-summary-row')?.dispatchEvent(new MouseEvent('click', { bubbles: true })))()`,
  );
  await session.wait(260);
  const selectedLayer = await session.evaluate(
    `(() => { const row = document.querySelector('.layer-summary-row.selected'); return row?.dataset.selectId || row?.dataset.selectionId || null })()`,
  );
  await session.evaluate(
    `(() => document.querySelector('.layer-summary-row')?.dispatchEvent(new MouseEvent('dblclick', { bubbles: true, cancelable: true })))()`,
  );
  await session.wait(30);
  const layerFilter = await text(session, "#layer-filter-summary");

  await click(session, "[data-preset='compute']");
  const compute = await visiblePanels(session);
  await click(session, "[data-pe-overview-mode='chart']");
  const peMode = await session.evaluate(
    `(() => ({ chart: !document.getElementById('pe-overview-chart').hidden, grid: !document.getElementById('pe-overview-grid').hidden }))()`,
  );

  await click(session, "[data-preset='layers']");
  await session.evaluate(
    `(() => { const select = document.getElementById('relationship-mode'); select.value = 'tensor-pe'; select.dispatchEvent(new Event('change', { bubbles: true })) })()`,
  );
  await session.wait(30);
  const relationships = await session.evaluate(
    `(() => ({ mode: document.getElementById('relationship-mode').value, rendered: Boolean(document.querySelector('.relationship-plot, .relationship-bundle .memory-empty')), status: [...(document.querySelector('.relationship-status')?.children || [])].map(element => element.textContent.replace(/\\s+/g, ' ').trim()) }))()`,
  );

  await click(session, "[data-preset='memory']");
  const memory = await panelState(session, ["#memory-summary", "#memories-overview", "#memory-detail", "#tensor-memory"]);
  await click(session, "[data-preset='tensor']");
  const tensor = await panelState(session, ["#tensor-memory", "#selected-tensor", "#pe-grid"]);

  await session.evaluate(
    `(() => { const layout = document.getElementById('view-layout'); layout.value = 'two'; layout.dispatchEvent(new Event('change', { bubbles: true })); document.querySelector('[data-view]:not([hidden]) .workspace-collapse')?.click() })()`,
  );
  const workspaceBefore = await workspaceState(session);
  await session.reload();
  const workspaceAfter = await workspaceState(session);
  await session.screenshot(screenshot);

  return {
    initial,
    visible_presets: { layers, compute },
    selection: { selected_layer: selectedLayer, filter_summary: layerFilter },
    pe_mode: peMode,
    relationships,
    memory,
    tensor,
    workspace_restored: workspaceBefore.layout === workspaceAfter.layout && workspaceBefore.collapsed === workspaceAfter.collapsed,
  };
}

async function snapshot(session) {
  return session.evaluate(
    `(() => ({
      ready: document.documentElement.dataset.gwrReady,
      stats: [...document.querySelectorAll('.stats strong, .stats em')].map(element => element.textContent.trim()),
      source: document.getElementById('source-path').textContent,
      visible: [...document.querySelectorAll('[data-view]:not([hidden])')].map(element => element.dataset.view),
      summaries: ['layer', 'pe', 'memory', 'tensor'].map(kind => document.getElementById(kind + '-filter-summary').textContent.trim()),
      workspace_bars: document.querySelectorAll('.workspace-panel-bar').length,
    }))()`,
  );
}

async function click(session, selector) {
  await session.evaluate(
    `(() => { const element = document.querySelector(${JSON.stringify(selector)}); if (!element) throw new Error(${JSON.stringify(`Missing ${selector}`)}); element.click() })()`,
  );
  await session.wait(30);
}

async function visiblePanels(session) {
  return session.evaluate(
    `(() => [...document.querySelectorAll('[data-view]:not([hidden])')].map(element => element.dataset.view))()`,
  );
}

async function panelState(session, selectors) {
  return session.evaluate(
    `(() => Object.fromEntries(${JSON.stringify(selectors)}.map(selector => { const element = document.querySelector(selector); return [selector, Boolean(element && element.textContent.trim())] })))()`,
  );
}

async function workspaceState(session) {
  return session.evaluate(
    `(() => ({ layout: document.getElementById('view-layout').value, collapsed: document.querySelector('[data-view].workspace-collapsed')?.dataset.view || null }))()`,
  );
}

async function text(session, selector) {
  return session.evaluate(
    `(() => document.querySelector(${JSON.stringify(selector)})?.textContent.trim() || '')()`,
  );
}

async function compareScreenshots(javascriptPath, wasmPath, diffPath) {
  const [javascript, wasm] = await Promise.all([
    readFile(javascriptPath).then((value) => PNG.sync.read(value)),
    readFile(wasmPath).then((value) => PNG.sync.read(value)),
  ]);
  if (javascript.width !== wasm.width || javascript.height !== wasm.height) {
    return { difference_pixels: Infinity, difference_ratio: 1, dimensions_match: false };
  }
  const diff = new PNG({ width: javascript.width, height: javascript.height });
  const pixels = pixelmatch(javascript.data, wasm.data, diff.data, javascript.width, javascript.height, { threshold: 0.1 });
  await writeFile(diffPath, PNG.sync.write(diff));
  return {
    difference_pixels: pixels,
    difference_ratio: pixels / (javascript.width * javascript.height),
    dimensions_match: true,
  };
}
