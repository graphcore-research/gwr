// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

// Run with GWR_PLAYWRIGHT_MODULE pointing to an installed Playwright package,
// followed by one or more generated report directories. No server is needed.
const assert = require("node:assert/strict");
const { pathToFileURL } = require("node:url");
const { resolve } = require("node:path");
const { chromium } = require(process.env.GWR_PLAYWRIGHT_MODULE || "playwright");

async function checkReport(browser, directory) {
  const page = await browser.newPage({
    viewport: { width: 1800, height: 1000 },
  });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const settle = () => page.waitForTimeout(150);
  const workspace = async (name) => {
    await page
      .locator(`#workspace-navigation [data-workspace="${name}"]`)
      .click();
    await settle();
  };
  try {
    await page.goto(pathToFileURL(resolve(directory, "index.html")).href);
    await settle();
    for (const width of [1800, 1100, 736, 360]) {
      await page.setViewportSize({ width, height: 1000 });
      const pickers = page.locator(".filter-controls > .filter-picker");
      for (let index = 0; index < (await pickers.count()); index++) {
        const picker = pickers.nth(index);
        const summary = picker.locator("summary");
        const before = await summary.boundingBox();
        await summary.click();
        await settle();
        const after = await summary.boundingBox();
        const menu = await picker.locator(".filter-dropdown").boundingBox();
        assert.deepEqual(
          after,
          before,
          "Opening a filter must not move its control",
        );
        assert.ok(Math.abs(menu.y - (after.y + after.height)) <= 2);
        assert.ok(menu.x >= 0 && menu.x + menu.width <= width);
        if (after.x + menu.width <= width - 12) {
          assert.ok(Math.abs(menu.x - after.x) <= 1);
        }
        await summary.click();
      }
    }
    await page.setViewportSize({ width: 1800, height: 1000 });
    await settle();
    assert.deepEqual(
      await page.evaluate(() => {
        const app = window.GWR_VISUALISATION_APP;
        const baseline = app.captureViewSettings();
        const legacy = structuredClone(baseline);
        legacy.peOverview.measure = "tensor:read";
        legacy.peOverview.chartColumns = [{ key: "tensor:write", width: 160 }];
        legacy.peOverview.chartSortKey = "tensor:write";
        app.restoreViewSettings(legacy);
        const result = {
          measure: app.peOverviewControls.measure.value,
          columns: app.peChartColumnConfiguration.snapshot(),
          sort: app.peOverviewControls.chartSortKey,
          selectedTensorOptions: [
            ...app.peOverviewControls.measure.options,
          ].filter((option) => option.value.startsWith("tensor:")).length,
        };
        app.restoreViewSettings(baseline);
        return result;
      }),
      {
        measure: "tensor:read",
        columns: [{ key: "tensor:write", width: 160 }],
        sort: "tensor:write",
        selectedTensorOptions: 2,
      },
    );
    assert.equal(
      await page
        .locator('[data-workspace="summary"]')
        .getAttribute("aria-pressed"),
      "true",
    );
    await page.evaluate(() => {
      const app = window.GWR_VISUALISATION_APP;
      window.renderCounts = {};
      for (const [id, view] of app.viewRegistry) {
        if (!view.render) continue;
        const render = view.render;
        view.render = () => {
          window.renderCounts[id] = (window.renderCounts[id] || 0) + 1;
          render();
        };
      }
    });
    for (const name of ["layers", "compute", "memory", "tensors", "dataflow"]) {
      await workspace(name);
      if (name === "layers") {
        const breakdown = await page.evaluate(() => {
          const app = window.GWR_VISUALISATION_APP;
          const configuration = app.layerOverviewColumnConfiguration;
          const saved = configuration.snapshot();
          const types = [
            ...app.computeNodeTypes.map((type) => ({
              ...type,
              prefix: "nodes",
              value: (aggregate) =>
                Number(aggregate.computeNodesByOp[type.name] || 0),
            })),
            ...app.machineOpTypes.map((type) => ({
              ...type,
              prefix: "ops",
              value: (aggregate) =>
                app.toBigInt(aggregate.machineOps[type.name]),
            })),
          ];
          const keys = types.map((type) => `${type.prefix}:${type.name}`);
          configuration.apply(keys);
          app.renderLayerSummary();
          const actual = [],
            expected = [];
          for (const row of document.querySelectorAll(".layer-summary-row")) {
            const aggregate = app.aggregateLayer(
              app.data.layers.find((layer) => layer.name === row.dataset.layer),
            );
            types.forEach((type, index) => {
              const definition = configuration.definitions.find(
                (column) => column.key === keys[index],
              );
              actual.push([
                row.children[index + 1].querySelector("strong").textContent,
                definition.colour,
              ]);
              expected.push([
                definition.format(type.value(aggregate)),
                type.colour,
              ]);
            });
          }
          configuration.restore(saved);
          app.renderLayerSummary();
          return { actual, expected };
        });
        assert.deepEqual(breakdown.actual, breakdown.expected);
        const row = page
          .locator(".workspace-primary .layer-summary-row")
          .last();
        await row.click();
        await page.waitForFunction(
          () =>
            document
              .querySelector(".workspace-primary .layer-summary-row:last-child")
              ?.getAttribute("aria-pressed") === "true",
        );
        const appearance = await row.evaluate((element) => {
          const style = getComputedStyle(element);
          const label = getComputedStyle(
            element.querySelector(".layer-overview-name"),
          );
          return {
            selected: element.getAttribute("aria-pressed"),
            width: style.outlineWidth,
            offset: style.outlineOffset,
            weight: label.fontWeight,
          };
        });
        assert.deepEqual(appearance, {
          selected: "true",
          width: "3px",
          offset: "-3px",
          weight: "700",
        });
        await page.screenshot({ path: "/tmp/gwr-layer-selection.png" });
      }
      if (name === "compute") {
        assert.equal(
          await page.evaluate(
            () => window.GWR_VISUALISATION_APP.peOverviewControls.mode,
          ),
          "chart",
        );
      }
      const table = page.locator(".workspace-primary .overview-table:visible");
      if (
        name === "tensors" &&
        (await page.evaluate(
          () => window.GWR_VISUALISATION_APP.data.tensors.length > 100,
        ))
      ) {
        await page.evaluate(() => {
          const app = window.GWR_VISUALISATION_APP;
          app.setTensorOverviewPageSize(100);
          app.resetTensorOverviewPage();
          app.markViewsDirty(["tensor-overview"]);
          app.scheduleVisibleViews();
        });
        await settle();
        const last = table.locator("[data-selection-id]").last();
        const id = await last.getAttribute("data-selection-id");
        await last.click();
        await page.waitForFunction(
          (id) => window.GWR_VISUALISATION_APP.state.selectedTensor?.id === id,
          id,
        );
        await last.press("ArrowDown");
        await page.waitForFunction(
          () => window.GWR_VISUALISATION_APP.tensorOverviewControls.page === 1,
        );
        await page.keyboard.press("ArrowUp");
        await page.waitForFunction(
          () => window.GWR_VISUALISATION_APP.tensorOverviewControls.page === 0,
        );
      }
      if (await table.count()) {
        const configurations = {
          layers: "layerOverviewColumnConfiguration",
          compute: "peChartColumnConfiguration",
          memory: "memoryOverviewColumnConfiguration",
          tensors: "tensorOverviewColumnConfiguration",
        };
        const configurationName = configurations[name];
        const originalColumns = await page.evaluate(
          (key) => window.GWR_VISUALISATION_APP[key].snapshot(),
          configurationName,
        );
        const picker = page.locator(
          ".workspace-primary .overview-column-picker:visible",
        );
        await picker.locator("summary").click();
        const pattern = picker.locator("[data-column-pattern]");
        await pattern.fill("read|written|machine");
        const matchingKeys = await picker
          .locator(".filter-option:visible input")
          .evaluateAll((inputs) => inputs.map((input) => input.value));
        assert.ok(matchingKeys.length);
        await picker
          .getByRole("button", { name: "Select matches", exact: true })
          .click();
        assert.deepEqual(
          await page.evaluate(
            (key) =>
              window.GWR_VISUALISATION_APP[key]
                .snapshot()
                .map((column) => column.key),
            configurationName,
          ),
          matchingKeys,
        );
        assert.equal(await pattern.inputValue(), "read|written|machine");
        await pattern.fill("[");
        await picker
          .getByRole("button", { name: "Select matches", exact: true })
          .click();
        assert.equal(await pattern.getAttribute("aria-invalid"), "true");
        assert.deepEqual(
          await page.evaluate(
            (key) =>
              window.GWR_VISUALISATION_APP[key]
                .snapshot()
                .map((column) => column.key),
            configurationName,
          ),
          matchingKeys,
        );
        await picker
          .getByRole("button", { name: "Clear", exact: true })
          .click();
        assert.equal(await pattern.inputValue(), "");
        await pattern.fill("^no_such_column$");
        await picker
          .getByRole("button", { name: "Select matches", exact: true })
          .click();
        assert.deepEqual(
          await page.evaluate(
            (key) => window.GWR_VISUALISATION_APP[key].snapshot(),
            configurationName,
          ),
          [],
        );
        await picker
          .getByRole("button", { name: "Clear", exact: true })
          .click();
        await picker.locator("summary").click();
        await page.evaluate(
          ({ key, columns }) => {
            const app = window.GWR_VISUALISATION_APP;
            app[key].restore(columns);
            app.markAllViewsDirty();
            app.scheduleVisibleViews();
          },
          { key: configurationName, columns: originalColumns },
        );
        await settle();
        const rows = table.locator("[data-selection-id]");
        const ids = await rows.evaluateAll((elements) =>
          elements.map((element) => element.dataset.selectionId),
        );
        await rows.first().click();
        await page.waitForFunction(
          (id) => window.GWR_VISUALISATION_APP.state.activeEntity?.id === id,
          ids[0],
        );
        await rows.first().press("ArrowDown");
        await page.waitForFunction(
          (id) => window.GWR_VISUALISATION_APP.state.activeEntity?.id === id,
          ids[Math.min(1, ids.length - 1)],
        );
        await page.keyboard.press("ArrowUp");
        await page.waitForFunction(
          (id) => window.GWR_VISUALISATION_APP.state.activeEntity?.id === id,
          ids[0],
        );
        await settle();
        const pageStep = await table.evaluate((element) =>
          Math.max(
            1,
            Math.floor(
              (element.clientHeight -
                element.firstElementChild.getBoundingClientRect().height) /
                element
                  .querySelector("[data-selection-id]")
                  .getBoundingClientRect().height,
            ),
          ),
        );
        await rows.first().press("PageDown");
        await page.waitForFunction(
          (id) => window.GWR_VISUALISATION_APP.state.activeEntity?.id === id,
          ids[Math.min(pageStep, ids.length - 1)],
        );
        await settle();
        await page.keyboard.press("PageUp");
        await page.waitForFunction(
          (id) => window.GWR_VISUALISATION_APP.state.activeEntity?.id === id,
          ids[0],
        );
        await settle();
        const dimensions = await table.evaluate((element) => {
          const table = element.getBoundingClientRect();
          const panel = element.parentElement.getBoundingClientRect();
          return {
            height: table.height,
            bottomGap: panel.bottom - table.bottom,
          };
        });
        assert.ok(
          dimensions.height > 540,
          `${name} table should use the tall region`,
        );
        assert.ok(
          dimensions.bottomGap <= 12,
          `${name} table should fill the region`,
        );
      }
      if (name === "tensors")
        await page.screenshot({ path: "/tmp/gwr-full-height-tables.png" });
    }
    await page.evaluate(() => {
      const app = window.GWR_VISUALISATION_APP;
      app.selectTensor(app.data.tensors[0]);
    });
    await settle();
    const connections = page.locator("#selected-node [data-node-neighbour]");
    assert.ok(await connections.count());
    const neighbour = await connections.first().evaluate((element) => ({
      kind: element.dataset.entityKind,
      id: element.dataset.entityId,
    }));
    for (const entity of [
      { kind: "tensor", id: "unrelated-tensor" },
      { kind: "compute", id: "unrelated-compute" },
      neighbour,
    ]) {
      await page.evaluate(
        (entity) =>
          window.GWR_VISUALISATION_APP.setHoveredEntity(entity.kind, entity.id),
        entity,
      );
      assert.equal(
        await connections.evaluateAll((elements) =>
          elements.every(
            (element) => getComputedStyle(element).opacity === "1",
          ),
        ),
        true,
      );
    }
    assert.equal(
      await connections
        .first()
        .evaluate((element) => element.classList.contains("entity-hovered")),
      true,
    );
    await page.evaluate(() =>
      window.GWR_VISUALISATION_APP.setHoveredEntity(null, null),
    );
    assert.equal(
      await connections
        .first()
        .evaluate((element) => element.classList.contains("entity-hovered")),
      false,
    );
    await page.locator("#timetable-graph-expand-all").click();
    await settle();
    const graphGeometry = () =>
      page.evaluate(() => {
        const app = window.GWR_VISUALISATION_APP;
        return {
          selection: app.state.activeEntity,
          zoom: {
            ...document.querySelector("#timetable-graph > canvas").__zoom,
          },
          nodes: app.graphSelectionModel().nodes.map((node) => ({
            id: node.id,
            kind: node.kind,
            expanded: node.expanded,
            radius: node.radius,
            width: node.halfWidth,
            height: node.halfHeight,
          })),
        };
      });
    const originalGeometry = await graphGeometry();
    const graphOptions = page.locator(".workspace-graph-options");
    await graphOptions.locator("summary").click();
    for (const scale of [0.1, 10, 1]) {
      for (const kind of ["tensor", "compute"]) {
        const input = page.locator(`#timetable-graph-${kind}-scale`);
        await input.fill(String(scale));
        await input.press("Enter");
      }
      const geometry = await graphGeometry();
      assert.deepEqual(geometry.selection, originalGeometry.selection);
      assert.deepEqual(geometry.zoom, originalGeometry.zoom);
      assert.deepEqual(
        geometry.nodes.map((n) => [n.id, n.expanded]),
        originalGeometry.nodes.map((n) => [n.id, n.expanded]),
      );
      for (const [index, node] of geometry.nodes.entries()) {
        const original = originalGeometry.nodes[index];
        const dimensions =
          node.kind === "tensor"
            ? ["width", "height"]
            : node.kind === "compute" ||
                (node.kind === "group" && !node.expanded)
              ? ["radius"]
              : [];
        for (const key of dimensions) {
          assert.ok(
            Math.abs(node[key] / original[key] - scale) < 1e-8,
            `${node.kind} ${key} should scale by ${scale}`,
          );
        }
      }
    }
    await graphOptions.locator("summary").click();
    const scales = await page.evaluate(() => {
      const app = window.GWR_VISUALISATION_APP;
      const original = app.captureViewSettings();
      app.setTimetableGraphNodeScale("tensor", 0.01);
      app.setTimetableGraphNodeScale("compute", 100);
      const custom = app.captureViewSettings();
      app.restoreViewSettings(original);
      app.restoreViewSettings(custom);
      const restored = app.captureViewSettings().timetableGraph;
      app.setTimetableGraphNodeScale("tensor", "invalid");
      const invalid = app.timetableGraphControls.tensorScale.value;
      app.restoreViewSettings(original);
      return {
        tensor: restored.tensorScale,
        compute: restored.computeScale,
        invalid,
      };
    });
    assert.deepEqual(scales, { tensor: "0.1", compute: "10", invalid: "1" });
    const graphCanvas = page.locator("#timetable-graph > canvas");
    const graphBounds = await graphCanvas.boundingBox();
    const zoomPoint = {
      x: Math.round(graphBounds.x + graphBounds.width * 0.25),
      y: Math.round(graphBounds.y + graphBounds.height * 0.25),
    };
    const zoomBefore = await graphCanvas.evaluate((canvas) => ({
      ...canvas.__zoom,
    }));
    await page.mouse.move(zoomPoint.x, zoomPoint.y);
    await page.mouse.wheel(0, -120);
    await settle();
    const zoomAfter = await graphCanvas.evaluate((canvas) => ({
      ...canvas.__zoom,
    }));
    assert.ok(zoomAfter.k > zoomBefore.k);
    const localZoomPoint = [
      zoomPoint.x - graphBounds.x,
      zoomPoint.y - graphBounds.y,
    ];
    for (const dimension of [0, 1]) {
      const beforeCoordinate =
        (localZoomPoint[dimension] -
          (dimension === 0 ? zoomBefore.x : zoomBefore.y)) /
        zoomBefore.k;
      const afterCoordinate =
        (localZoomPoint[dimension] -
          (dimension === 0 ? zoomAfter.x : zoomAfter.y)) /
        zoomAfter.k;
      assert.ok(Math.abs(afterCoordinate - beforeCoordinate) < 1e-6);
    }
    const selectionBeforePan = await page.evaluate(
      () => window.GWR_VISUALISATION_APP.state.activeEntity,
    );
    await page.mouse.down();
    await page.mouse.move(zoomPoint.x + 48, zoomPoint.y + 30);
    await page.mouse.up();
    await settle();
    const panAfter = await graphCanvas.evaluate((canvas) => ({
      ...canvas.__zoom,
    }));
    assert.ok(Math.abs(panAfter.x - zoomAfter.x - 48) < 1e-6);
    assert.ok(Math.abs(panAfter.y - zoomAfter.y - 30) < 1e-6);
    assert.equal(panAfter.k, zoomAfter.k);
    assert.deepEqual(
      await page.evaluate(
        () => window.GWR_VISUALISATION_APP.state.activeEntity,
      ),
      selectionBeforePan,
      "Panning must not select a graph node",
    );
    const selection = await page.evaluate(() => {
      const app = window.GWR_VISUALISATION_APP;
      app.selectPe(app.data.pes[0]);
      return app.data.pes[0].name;
    });
    await settle();
    assert.deepEqual(
      await page.evaluate(
        () => window.GWR_VISUALISATION_APP.state.activeEntity,
      ),
      { kind: "pe", id: selection },
    );
    assert.equal(
      await page
        .locator("#workspace-inspector-content")
        .getAttribute("data-kind"),
      "pe",
    );
    const before = await page
      .locator("#timetable-graph > canvas")
      .evaluate((c) => ({ ...c.__zoom }));
    await page
      .locator(".workspace-primary")
      .getByRole("button", { name: "Maximize", exact: true })
      .click();
    await settle();
    await page
      .locator(".workspace-primary")
      .getByRole("button", { name: "Restore", exact: true })
      .click();
    await settle();
    assert.deepEqual(
      await page
        .locator("#timetable-graph > canvas")
        .evaluate((c) => ({ ...c.__zoom })),
      before,
    );
    const navigator = page.locator("#timetable-graph-overview canvas");
    assert.equal(
      await page.locator('[data-view="timetable-graph-overview"]').count(),
      0,
    );
    const bounds = await navigator.boundingBox();
    assert.ok(bounds.width <= 240 && bounds.height <= 160);
    await navigator.click({
      position: { x: bounds.width * 0.2, y: bounds.height * 0.4 },
    });
    await settle();
    assert.notDeepEqual(
      await page
        .locator("#timetable-graph > canvas")
        .evaluate((c) => ({ ...c.__zoom })),
      before,
    );
    const oldScale = await page
      .locator("#timetable-graph > canvas")
      .evaluate((c) => c.__zoom.k);
    await navigator.press("+");
    await page.waitForTimeout(350);
    assert.ok(
      (await page
        .locator("#timetable-graph > canvas")
        .evaluate((c) => c.__zoom.k)) > oldScale,
    );
    await page.screenshot({ path: "/tmp/gwr-embedded-navigator.png" });
    for (const [width, height, layout] of [
      [1600, 1100, "beside"],
      [1600, 1300, "below"],
      [1800, 900, "beside"],
      [1100, 900, "below"],
      [1100, 550, "tabs"],
      [736, 900, "tabs"],
      [360, 900, "tabs"],
    ]) {
      await page.setViewportSize({ width, height });
      await settle();
      assert.equal(
        await page.locator("#workspace-shell").getAttribute("data-arrangement"),
        layout,
      );
      assert.equal(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth,
        ),
        true,
      );
      const settings = page.locator(".workspace-graph-options > summary");
      for (const id of ["reset", "expand-visible", "expand-all", "collapse"]) {
        const action = page.locator(
          `.timetable-graph-controls > #timetable-graph-${id}`,
        );
        assert.equal(await action.isVisible(), true);
      }
      await settings.click();
      await settle();
      const menu = page.locator(".workspace-graph-options-body");
      const menuBounds = await menu.boundingBox();
      const panelBounds = await page
        .locator('[data-view="timetable-graph"]')
        .boundingBox();
      assert.ok(menuBounds.x >= panelBounds.x);
      assert.ok(
        menuBounds.x + menuBounds.width <= panelBounds.x + panelBounds.width,
      );
      assert.ok(
        menuBounds.y + menuBounds.height <= panelBounds.y + panelBounds.height,
      );
      await page.locator("#timetable-graph-edges-all").scrollIntoViewIfNeeded();
      const lastControl = await page
        .locator("#timetable-graph-edges-all")
        .boundingBox();
      assert.ok(
        lastControl.y >= menuBounds.y &&
          lastControl.y + lastControl.height <=
            menuBounds.y + menuBounds.height,
      );
      if (width === 360)
        await page.screenshot({ path: "/tmp/gwr-graph-settings-narrow.png" });
      await settings.click();
    }
    await page.setViewportSize({ width: 1800, height: 1000 });
    await settle();
    const divider = page.getByRole("separator", {
      name: "Primary and companion split",
    });
    const initialRatio = await divider.getAttribute("aria-valuenow");
    await divider.press("ArrowLeft");
    assert.notEqual(await divider.getAttribute("aria-valuenow"), initialRatio);
    await divider.dblclick();
    assert.equal(await divider.getAttribute("aria-valuenow"), initialRatio);
    await page.locator(".workspace-primary summary").first().click();
    await page
      .getByLabel("Add view to primary")
      .selectOption("tensor-overview");
    await page
      .locator(".workspace-primary")
      .getByRole("button", { name: "Add view", exact: true })
      .click();
    assert.equal(
      await page.locator('[data-view="tensor-overview"]').count(),
      1,
    );
    await page.locator(".workspace-primary summary").first().click();
    await page
      .getByRole("button", { name: "Move to other region", exact: true })
      .click();
    assert.equal(
      await page
        .locator('.workspace-companion [data-view="tensor-overview"]')
        .count(),
      1,
    );
    await workspace("compute");
    await page.evaluate(() => {
      const a = window.GWR_VISUALISATION_APP;
      a.setPeOverviewMode("chart");
      a.workspaceChanged();
    });
    await workspace("tensors");
    assert.equal(
      await page.evaluate(
        () => window.GWR_VISUALISATION_APP.peOverviewControls.mode,
      ),
      "grid",
    );
    await workspace("compute");
    assert.equal(
      await page.evaluate(
        () => window.GWR_VISUALISATION_APP.peOverviewControls.mode,
      ),
      "chart",
    );
    await page.evaluate(() => {
      const a = window.GWR_VISUALISATION_APP;
      a.selectTensor(a.data.tensors[0]);
    });
    await settle();
    assert.equal(
      await page
        .locator("#workspace-inspector-content")
        .getAttribute("data-kind"),
      "tensor",
    );
    await workspace("tensors");
    await page
      .locator('.workspace-companion [role="tab"]')
      .filter({ hasText: "Tensor memory map" })
      .click();
    await settle();
    const memoryDimensions = await page
      .locator("#tensor-memory")
      .evaluate((element) => {
        const bounds = element.getBoundingClientRect();
        const panel = element.closest("[data-view]").getBoundingClientRect();
        return {
          height: bounds.height,
          bottomGap: panel.bottom - bounds.bottom,
        };
      });
    assert.ok(memoryDimensions.height > 460);
    assert.ok(memoryDimensions.bottomGap <= 12);
    await page.locator("#tensor-memory").evaluate((element) => {
      element.style.maxHeight = "80px";
    });
    await page.addStyleTag({
      content: "#workspace-shell .tensor-overview-table { max-height: 160px; }",
    });
    for (const method of ["selectTensor", "selectGraphTensor"]) {
      await page.evaluate((method) => {
        const app = window.GWR_VISUALISATION_APP;
        const map = document.getElementById("tensor-memory");
        map.scrollTop = 0;
        const rows = map.querySelectorAll(".memory-tensor-row");
        const id = rows[rows.length - 1].dataset.selectionId;
        app[method](app.data.tensors.find((tensor) => tensor.id === id));
      }, method);
      await settle();
      assert.equal(
        await page.locator("#tensor-memory").evaluate((element) => {
          const row = element
            .querySelector(".memory-tensor-row.selected")
            .getBoundingClientRect();
          const bounds = element.getBoundingClientRect();
          return (
            row.top >= bounds.top - 1 &&
            (row.bottom <= bounds.bottom + 1 ||
              row.height > element.clientHeight)
          );
        }),
        true,
      );
      for (const selector of ["#tensor-memory", ".tensor-overview-table"]) {
        assert.equal(
          await page.locator(selector).evaluate((element) => {
            const row = element
              .querySelector(".selected")
              .getBoundingClientRect();
            const header = element.querySelector(".tensor-overview-header");
            const top =
              element.getBoundingClientRect().top +
              (header?.getBoundingClientRect().height || 0);
            return (
              Math.abs(row.top - top) <= 1 ||
              element.scrollTop >=
                element.scrollHeight - element.clientHeight - 1
            );
          }),
          true,
        );
      }
    }
    await page.evaluate(() => {
      const a = window.GWR_VISUALISATION_APP;
      a.filterState.tensors.clear();
      a.filtersChanged();
    });
    await settle();
    assert.equal(
      await page
        .locator("#workspace-inspector-content")
        .getAttribute("data-kind"),
      "empty",
    );
    await workspace("summary");
    await page.evaluate(() => {
      window.renderCounts = {};
      window.GWR_VISUALISATION_APP.markAllViewsDirty();
      window.GWR_VISUALISATION_APP.scheduleVisibleViews();
    });
    await settle();
    const counts = await page.evaluate(() => window.renderCounts);
    assert.deepEqual(Object.keys(counts).sort(), [
      "compute-summary",
      "memory-summary",
      "timetable-summary",
    ]);
    await workspace("compute");
    await page.reload();
    await settle();
    assert.equal(
      await page
        .locator('[data-workspace="compute"]')
        .getAttribute("aria-pressed"),
      "true",
    );
    await page.addInitScript(() => {
      Storage.prototype.getItem = () => {
        throw new Error("Storage disabled");
      };
      Storage.prototype.setItem = () => {
        throw new Error("Storage disabled");
      };
    });
    await page.reload();
    await settle();
    assert.equal(
      await page
        .locator('[data-workspace="summary"]')
        .getAttribute("aria-pressed"),
      "true",
    );
    assert.deepEqual(errors, []);
    console.log(`Workspace browser checks passed: ${directory}`);
  } finally {
    await page.close();
  }
}

(async () => {
  assert.ok(process.argv.length > 2, "Provide generated report directories");
  const browser = await chromium.launch({ headless: true, channel: "chrome" });
  try {
    for (const directory of process.argv.slice(2))
      await checkReport(browser, directory);
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
