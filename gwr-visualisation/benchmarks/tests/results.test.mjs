// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { readConfig } from "../lib/config.mjs";
import {
  compareBaseline,
  summarizeSamples,
  validateChecksums,
  writeResults,
} from "../lib/results.mjs";
import { metricLabels } from "../lib/scenarios.mjs";

const matchingMetadata = {
  operating_system: "Test OS",
  hardware: "Test CPU",
  browsers: { chromium: "1" },
  workload: {
    timetable: { sha256: "timetable" },
    platform: null,
    overlay: null,
    kernel_iterations: 3,
    interaction_layer_pattern: "^layer",
  },
};

test("summarizeSamples reports measured medians and ignores warm-ups", () => {
  const samples = [
    sample(1_000, true),
    sample(10, false),
    sample(30, false),
    sample(20, false),
    sample(40, false),
  ];

  assert.deepStrictEqual(summarizeSamples(samples).chromium, timings(25));
});

test("validateChecksums accepts stable kernel results", () => {
  const samples = [sample(10, true), sample(20, false)];

  assert.deepStrictEqual(validateChecksums(samples), {
    passed: true,
    failures: [],
  });
});

test("validateChecksums rejects a changed kernel result", () => {
  const samples = [sample(10, false), sample(20, false)];
  samples[1].checksums.geometry += 1;

  const result = validateChecksums(samples);

  assert.equal(result.passed, false);
  assert.match(result.failures[0], /geometry checksum was not stable/);
});

test("compareBaseline does not gate runs without a baseline", () => {
  const result = compareBaseline(
    { chromium: timings(100) },
    matchingMetadata,
    null,
    10,
  );

  assert.equal(result.enabled, false);
  assert.equal(result.passed, true);
});

test("compareBaseline accepts improvements and the configured limit", () => {
  const current = timings(90);
  current.cold_startup_ms = 110;

  const result = compareBaseline(
    { chromium: current },
    matchingMetadata,
    baseline(timings(100)),
    10,
  );

  assert.equal(result.passed, true);
  assert.equal(result.comparisons.chromium.cold_startup_ms.change_percent, 10);
});

test("compareBaseline rejects regressions over the configured limit", () => {
  const current = timings(100);
  current.relationships_ms = 110.1;

  const result = compareBaseline(
    { chromium: current },
    matchingMetadata,
    baseline(timings(100)),
    10,
  );

  assert.equal(result.passed, false);
  assert.match(result.failures[0], /Relationship build \+ render regressed/);
});

test("compareBaseline requires matching browser and metric entries", () => {
  const missingBrowser = compareBaseline(
    { safari: timings(100) },
    matchingMetadata,
    baseline(timings(100)),
    10,
  );
  const incomplete = timings(100);
  delete incomplete.kernel_geometry_ms;
  const missingMetric = compareBaseline(
    { chromium: timings(100) },
    matchingMetadata,
    baseline(incomplete),
    10,
  );

  assert.match(missingBrowser.failures[0], /no 'safari' browser results/);
  assert.match(missingMetric.failures[0], /no valid Geometry kernel metric/);
});

test("compareBaseline reports environment differences as warnings", () => {
  const metadata = {
    operating_system: "New OS",
    hardware: "New CPU",
    browsers: { chromium: "2" },
    workload: matchingMetadata.workload,
  };

  const result = compareBaseline(
    { chromium: timings(100) },
    metadata,
    baseline(timings(100)),
    10,
  );

  assert.equal(result.passed, true);
  assert.equal(result.warnings.length, 3);
  assert.match(result.warnings[0], /Operating system differs/);
});

test("compareBaseline rejects a different workload", () => {
  const metadata = {
    ...matchingMetadata,
    workload: {
      ...matchingMetadata.workload,
      timetable: { sha256: "different" },
      kernel_iterations: 4,
    },
  };

  const result = compareBaseline(
    { chromium: timings(100) },
    metadata,
    baseline(timings(100)),
    10,
  );

  assert.equal(result.passed, false);
  assert.deepStrictEqual(result.failures, [
    "Timetable differs from baseline",
    "Kernel iterations differs from baseline",
  ]);
  assert.deepStrictEqual(result.comparisons, {});
});

test("readConfig accepts an explicit baseline and regression limit", () => {
  const config = readConfig([
    "--timetable",
    "/tmp/timetable.yaml",
    "--baseline",
    "baseline.json",
    "--max-regression-percent",
    "12.5",
  ]);

  assert.equal(config.baseline, path.resolve("baseline.json"));
  assert.equal(config.maxRegressionPercent, 12.5);
});

test("readConfig rejects a negative regression limit", () => {
  assert.throws(
    () =>
      readConfig([
        "--timetable",
        "/tmp/timetable.yaml",
        "--max-regression-percent",
        "-1",
      ]),
    /Expected a non-negative number/,
  );
});

test("readConfig rejects fractional integer options", () => {
  assert.throws(
    () => readConfig(["--timetable", "/tmp/timetable.yaml", "--runs", "1.5"]),
    /Expected a positive integer/,
  );
});

test("readConfig rejects unknown and duplicate options", () => {
  assert.throws(
    () =>
      readConfig([
        "--timetable",
        "/tmp/timetable.yaml",
        "--basline",
        "old.json",
      ]),
    /Unknown option: --basline/,
  );
  assert.throws(
    () =>
      readConfig([
        "--timetable",
        "/tmp/one.yaml",
        "--timetable",
        "/tmp/two.yaml",
      ]),
    /Option supplied more than once: --timetable/,
  );
});

test("writeResults compares an explicit JSON baseline", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "gwr-benchmark-results-"));
  const baselinePath = path.join(root, "baseline.json");
  const output = path.join(root, "output");
  await writeFile(
    baselinePath,
    JSON.stringify({
      schema_version: 2,
      metadata: matchingMetadata,
      summary: { chromium: timings(100) },
    }),
  );

  try {
    const result = await writeResults(
      {
        output,
        baseline: baselinePath,
        maxRegressionPercent: 10,
      },
      matchingMetadata,
      [sample(100, false)],
    );
    const written = JSON.parse(
      await readFile(path.join(output, "benchmark.json"), "utf8"),
    );

    assert.equal(result.passed, true);
    assert.equal(written.schema_version, 2);
    assert.equal(written.regression.enabled, true);
    assert.equal(written.regression.passed, true);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

function sample(value, warmup) {
  return {
    browser: "chromium",
    warmup,
    metrics: timings(value),
    checksums: { filtering: 1, aggregation: 2, geometry: 3 },
  };
}

function timings(value) {
  return Object.fromEntries(
    Object.keys(metricLabels).map((metric) => [metric, value]),
  );
}

function baseline(summary) {
  return {
    metadata: matchingMetadata,
    summary: { chromium: summary },
  };
}
