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
  trafficForAccesses,
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

test("trafficForAccesses intersects exact view and memory ranges", () => {
  const traffic = trafficForAccesses(
    [
      {
        layer: "layer 1",
        ranges: [
          { addr: "2", num_bytes: "2" },
          { addr: "8", num_bytes: "2" },
          { addr: "14", num_bytes: "2" },
        ],
      },
      {
        layer: "layer 2",
        ranges: [{ addr: "0", num_bytes: "4" }],
      },
    ],
    new Set(["layer 1"]),
    [
      [0n, 4n],
      [8n, 12n],
    ],
  );

  assert.equal(traffic.bytes, 4n);
  assert.equal(traffic.edgeCount, 1);
});

test("trafficForAccesses keeps separate edges distinct", () => {
  const traffic = trafficForAccesses([
    {
      ranges: [
        { addr: "0", num_bytes: "1" },
        { addr: "2", num_bytes: "1" },
      ],
    },
    { ranges: [{ addr: "4", num_bytes: "1" }] },
  ]);

  assert.equal(traffic.bytes, 3n);
  assert.equal(traffic.edgeCount, 2);
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

test("contextTensorCount uses the filtered tensor context", () => {
  assert.equal(contextTensorCount({ tensors: [{ id: "visible" }] }), 1);
  assert.equal(contextTensorCount({ tensors: [] }), 0);
});
