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
      { addr: "2", num_bytes: "8", layer: "layer 1" },
      { addr: "0", num_bytes: "4", layer: "layer 2" },
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
