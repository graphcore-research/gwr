// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

const assert = require("node:assert/strict");
const test = require("node:test");
global.window = { GWR_VISUALISATION_APP: {} };
require("../assets/workspace-model.js");
const {
  createWorkspace,
  workspaceDefinitions,
  openWorkspaceView,
  changeWorkspaceTab,
  workspaceArrangementFor,
  restoreWorkspaceModel,
} = window.GWR_VISUALISATION_APP;
const valid = new Set([
  "summary",
  "timetable-summary",
  "compute-summary",
  "memory-summary",
  ...Object.values(workspaceDefinitions).flatMap((value) => [
    ...value.primary,
    ...value.companion,
  ]),
]);

test("six guided workspaces start with independent state", () => {
  assert.equal(Object.keys(workspaceDefinitions).length, 6);
  const settings = { peOverview: { mode: "grid" } };
  const a = createWorkspace("compute", settings),
    b = createWorkspace("compute", settings);
  a.settings.peOverview.mode = "chart";
  a.companion.reverse();
  assert.equal(b.settings.peOverview.mode, "grid");
  assert.equal(b.companion[0], "relationships");
  assert.deepEqual(createWorkspace("summary").primary, ["summary"]);
  for (const name of Object.keys(workspaceDefinitions)) {
    const config = createWorkspace(name);
    assert.equal(config.horizontalRatio, 0.5);
    assert.equal(config.verticalRatio, 0.5);
    assert.equal(config.inspectorWidth, 360);
  }
});

test("adding an existing view activates its current region without duplication", () => {
  const c = createWorkspace("dataflow");
  assert.equal(openWorkspaceView(c, "primary", "relationships"), "companion");
  assert.equal(c.activeCompanion, "relationships");
  assert.ok(!c.primary.includes("relationships"));
});

test("tab operations preserve a primary view and repair active tabs", () => {
  const c = createWorkspace("compute");
  assert.equal(changeWorkspaceTab(c, "pe-grid", "close"), false);
  assert.equal(changeWorkspaceTab(c, "pe-grid", "move"), false);
  openWorkspaceView(c, "primary", "tensor-overview");
  changeWorkspaceTab(c, "tensor-overview", "earlier");
  assert.equal(c.primary[0], "tensor-overview");
  changeWorkspaceTab(c, "tensor-overview", "move");
  assert.equal(c.activePrimary, "pe-grid");
  assert.equal(c.activeCompanion, "tensor-overview");
  for (const id of [...c.companion]) changeWorkspaceTab(c, id, "close");
  assert.equal(c.activeCompanion, null);
});

test("combined and individual summaries never duplicate DOM ownership", () => {
  const c = createWorkspace("summary");
  openWorkspaceView(c, "companion", "compute-summary");
  assert.deepEqual(c.primary, ["compute-summary"]);
  openWorkspaceView(c, "companion", "summary");
  assert.deepEqual(c.primary, ["summary"]);
  assert.deepEqual(c.companion, []);
});

test("automatic layout handles wide, medium, narrow and short windows", () => {
  const c = createWorkspace("compute");
  assert.equal(workspaceArrangementFor(c, 1600, 800), "beside");
  assert.equal(workspaceArrangementFor(c, 1499, 1000), "below");
  assert.equal(workspaceArrangementFor(c, 1500, 1000), "beside");
  assert.equal(workspaceArrangementFor(c, 1600, 1000), "beside");
  assert.equal(workspaceArrangementFor(c, 2000, 1400), "below");
  assert.equal(workspaceArrangementFor(c, 2200, 1100), "beside");
  assert.equal(workspaceArrangementFor(c, 1100, 800), "below");
  assert.equal(workspaceArrangementFor(c, 736, 800), "tabs");
  assert.equal(workspaceArrangementFor(c, 1100, 450), "tabs");
  c.arrangement = "beside";
  assert.equal(workspaceArrangementFor(c, 1600, 1000), "beside");
  assert.equal(workspaceArrangementFor(c, 1000, 800), "tabs");
  c.inspector = false;
  assert.equal(workspaceArrangementFor(c, 1000, 800), "beside");
  assert.equal(c.arrangement, "beside");
});

test("storage migration imports settings but drops panel geometry", () => {
  const legacy = {
    version: 1,
    panels: { "pe-grid": { height: "800px" } },
    peOverview: { mode: "chart" },
  };
  const restored = restoreWorkspaceModel(legacy, valid);
  assert.equal(restored.active, "summary");
  assert.equal(restored.workspaces.compute.settings.peOverview.mode, "chart");
  assert.equal(restored.workspaces.compute.panels, undefined);
  restored.workspaces.compute.settings.peOverview.mode = "grid";
  assert.equal(legacy.peOverview.mode, "chart");
});

test("saved tabs and active workspace restore with invalid entries removed", () => {
  const source = {
    version: 2,
    active: "dataflow",
    workspaces: {
      dataflow: {
        primary: ["removed", "timetable-graph"],
        companion: [
          "timetable-graph-overview",
          "timetable-graph",
          "relationships",
        ],
        activeCompanion: "timetable-graph-overview",
        horizontalRatio: 4,
      },
    },
  };
  const result = restoreWorkspaceModel(source, valid);
  assert.equal(result.active, "dataflow");
  assert.deepEqual(result.workspaces.dataflow.primary, ["timetable-graph"]);
  assert.deepEqual(result.workspaces.dataflow.companion, ["relationships"]);
  assert.equal(result.workspaces.dataflow.activeCompanion, "relationships");
  assert.equal(result.workspaces.dataflow.horizontalRatio, 0.8);
  assert.equal(restoreWorkspaceModel(null, valid).active, "summary");
});
