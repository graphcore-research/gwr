// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const context = { window: {} };
vm.runInNewContext(
  readFileSync(new URL("../assets/view-model.js", import.meta.url), "utf8"),
  context,
);
const {
  contextTensorCount,
  rangeUnionBytes,
  retainedFocus,
  selectedWindow,
  strongestEdges,
  trafficForTransfers,
} =
  context.window.GWR_VISUALISATION_VIEW_MODEL;

test("rangeUnionBytes counts aliased allocations once", () => {
  assert.equal(
    rangeUnionBytes([
      [0n, 8n],
      [4n, 12n],
      [16n, 20n],
    ]),
    16n,
  );
});

test("trafficForTransfers intersects exact view and memory ranges", () => {
  const traffic = trafficForTransfers(
    [
      {
        layer: "layer 1",
        access: {
          first_element: "2",
          elements_per_range: "2",
          strides: [{ count: "3", stride_elements: "6" }],
          bits_per_element: "8",
          num_access_bytes: "6",
        },
      },
      {
        layer: "layer 2",
        access: {
          first_element: "0",
          elements_per_range: "4",
          strides: [],
          bits_per_element: "8",
          num_access_bytes: "4",
        },
      },
    ],
    "0",
    new Set(["layer 1"]),
    [
      [0n, 4n],
      [8n, 12n],
    ],
  );

  assert.equal(traffic.bytes, 4n);
  assert.equal(traffic.edgeCount, 1);
});

test("trafficForTransfers keeps separate edges distinct", () => {
  const traffic = trafficForTransfers(
    [
      {
        access: {
          first_element: "0",
          elements_per_range: "1",
          strides: [{ count: "2", stride_elements: "2" }],
          bits_per_element: "8",
          num_access_bytes: "2",
        },
      },
      {
        access: {
          first_element: "4",
          elements_per_range: "1",
          strides: [],
          bits_per_element: "8",
          num_access_bytes: "1",
        },
      },
    ],
    "0",
  );

  assert.equal(traffic.bytes, 3n);
  assert.equal(traffic.edgeCount, 2);
});

test("trafficForTransfers handles large strided views analytically", () => {
  const traffic = trafficForTransfers(
    [
      {
        access: {
          first_element: "0",
          elements_per_range: "1",
          strides: [{ count: "100000000", stride_elements: "2" }],
          bits_per_element: "8",
          num_access_bytes: "100000000",
        },
      },
    ],
    "0",
    null,
    [[199999998n, 200000000n]],
  );

  assert.equal(traffic.bytes, 1n);
  assert.equal(traffic.edgeCount, 1);
});

test("trafficForTransfers preserves packed byte intersections", () => {
  const traffic = trafficForTransfers(
    [
      {
        access: {
          first_element: "5",
          elements_per_range: "1",
          strides: [{ count: "3", stride_elements: "4" }],
          bits_per_element: "4",
          num_access_bytes: "3",
        },
      },
    ],
    "0",
    null,
    [[4n, 5n]],
  );

  assert.equal(traffic.bytes, 1n);
  assert.equal(traffic.edgeCount, 1);
});

test("retainedFocus clears a focus hidden by a preset", () => {
  assert.equal(
    retainedFocus("memory-details", new Set(["timetable-summary"])),
    null,
  );
  assert.equal(
    retainedFocus("memory-details", new Set(["memory-details"])),
    "memory-details",
  );
});

test("selectedWindow retains a selected value outside the window", () => {
  const values = Array.from({ length: 600 }, (_, index) => ({ id: index }));
  const window = selectedWindow(values, 500, 550, (value) => value.id);

  assert.equal(window.length, 500);
  assert.equal(window.at(-1).id, 550);
});

test("strongestEdges retains the strongest values", () => {
  const edges = Array.from({ length: 5_001 }, (_, index) => ({
    source: `source-${index}`,
    target: "target",
    value: BigInt(index + 1),
  }));
  const retained = strongestEdges(edges, 5_000);

  assert.equal(retained.length, 5_000);
  assert.equal(retained[0].value, 5_001n);
  assert.equal(retained.at(-1).value, 2n);
});

test("strongestEdges reserves the selected source", () => {
  const edges = [
    { source: "strongest", target: "target", value: 3n },
    { source: "second", target: "target", value: 2n },
    { source: "selected", target: "target", value: 1n },
  ];
  const retained = strongestEdges(edges, 2, "selected");

  assert.equal(
    retained.map((edge) => edge.source).join(","),
    "strongest,selected",
  );
});

test("contextTensorCount uses the filtered tensor context", () => {
  assert.equal(contextTensorCount({ tensors: [{ id: "visible" }] }), 1);
  assert.equal(contextTensorCount({ tensors: [] }), 0);
});
