// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

const assert = require("node:assert/strict");
const test = require("node:test");

class Element {
  constructor() {
    this.children = [];
    this.listeners = new Map();
    this.open = false;
    this.style = { setProperty() {} };
  }

  addEventListener(type, listener) {
    const listeners = this.listeners.get(type) || [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  dispatch(type) {
    for (const listener of this.listeners.get(type) || []) listener();
  }

  append(...children) {
    this.children.push(...children);
  }

  closest() {
    return this.details;
  }

  querySelector() {
    return this.menu;
  }

  querySelectorAll() {
    return [];
  }

  getBoundingClientRect() {
    return { left: 0 };
  }

  focus() {}
}

function loadFilters() {
  const names = ["layer", "pe", "memory", "tensor"];
  const controls = {};
  const details = {};
  const elements = new Map();
  for (const name of names) {
    const container = new Element();
    const picker = new Element();
    picker.menu = { offsetWidth: 200, style: { setProperty() {} } };
    container.details = picker;
    controls[name] = container;
    controls[`${name}Summary`] = new Element();
    details[name] = picker;
    for (const suffix of [
      "filter-pattern",
      "filter-pattern-status",
      "filter-select-matches",
      "filter-clear-pattern",
    ]) {
      elements.set(`${name}-${suffix}`, new Element());
    }
  }
  global.document = {
    documentElement: { clientWidth: 1200 },
    createElement: () => new Element(),
    getElementById: (id) => elements.get(id),
  };
  global.window = {
    addEventListener() {},
    GWR_VISUALISATION_APP: {
      data: {
        layers: [{ name: "layer 1" }, { name: "layer 2" }],
        pes: [{ name: "pe0" }],
        memory: { platform_memories: [{ name: "hbm0" }] },
        tensors: [{ id: "tensor0" }, { id: "tensor1" }],
      },
      fmt: new Intl.NumberFormat("en"),
      controls,
      filterContextCache: new Map(),
      machineOpKeys: [],
      emptyMachineOps: () => ({ total: 0n }),
      toBigInt: (value) => BigInt(value || 0),
      bigIntMax: (left, right) => (left > right ? left : right),
      bigIntToNumber: Number,
      scaleInteger: (value) => value,
      overlapBytes: () => 0n,
      matchingPickerValues: () => [],
    },
  };
  const path = require.resolve("../assets/filters.js");
  delete require.cache[path];
  require(path);
  return { app: window.GWR_VISUALISATION_APP, controls, details };
}

test("filter options are created only when their picker opens", () => {
  const { app, controls, details } = loadFilters();

  app.initializeFilterControls();

  for (const name of ["layer", "pe", "memory", "tensor"])
    assert.equal(controls[name].children.length, 0);

  details.layer.open = true;
  details.layer.dispatch("toggle");
  assert.equal(controls.layer.children.length, 2);
  assert.equal(controls.pe.children.length, 0);
});

test("unfiltered layer traffic uses its precomputed aggregate", () => {
  const { app } = loadFilters();
  let filteredCalls = 0;

  const traffic = app.layerOverviewTraffic(
    {
      tensor_read_bytes: "120",
      tensor_write_bytes: "45",
      tensor_count: 7,
    },
    true,
    () => {
      filteredCalls += 1;
      return { readBytes: 1n, writeBytes: 2n, tensors: ["filtered"] };
    },
  );

  assert.deepEqual(traffic, {
    readBytes: 120n,
    writeBytes: 45n,
    tensorCount: 7,
  });
  assert.equal(filteredCalls, 0);
});

test("filtered layer traffic preserves context-sensitive values", () => {
  const { app } = loadFilters();

  const traffic = app.layerOverviewTraffic({}, false, () => ({
    readBytes: 8n,
    writeBytes: 3n,
    tensors: [{ id: "a" }, { id: "b" }],
  }));

  assert.deepEqual(traffic, {
    readBytes: 8n,
    writeBytes: 3n,
    tensorCount: 2,
  });
});
