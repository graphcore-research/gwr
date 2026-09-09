// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

const assert = require("node:assert/strict");
const test = require("node:test");

global.window = { GWR_VISUALISATION_APP: {} };
require("../assets/palettes.js");

const { stableDataTypeColours } = window.GWR_VISUALISATION_APP;

test("keeps known data-type colours stable across reports", () => {
  const first = stableDataTypeColours(["fp32", "int8", "bf16"]);
  const second = stableDataTypeColours(["complex128", "int8", "fp32"]);

  assert.equal(first.get("fp32"), second.get("fp32"));
  assert.equal(first.get("int8"), second.get("int8"));
  assert.notEqual(first.get("fp32"), first.get("int8"));
});

test("assigns aliases their canonical data-type colour", () => {
  const colours = stableDataTypeColours([
    "fp16",
    "float16",
    "bf16",
    "bfloat16",
    "qint8",
    "int8",
  ]);

  assert.equal(colours.get("fp16"), colours.get("float16"));
  assert.equal(colours.get("bf16"), colours.get("bfloat16"));
  assert.equal(colours.get("qint8"), colours.get("int8"));
});

test("falls back to deterministic report-local colours for unknown types", () => {
  const first = stableDataTypeColours(["custom_z", "custom_a"]);
  const second = stableDataTypeColours(["custom_a", "custom_z"]);

  assert.equal(first.get("custom_a"), second.get("custom_a"));
  assert.equal(first.get("custom_z"), second.get("custom_z"));
  assert.notEqual(first.get("custom_a"), first.get("custom_z"));
});
