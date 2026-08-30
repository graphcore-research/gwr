// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";

import { metricLabels } from "./scenarios.mjs";

const execute = promisify(execFile);
const schemaVersion = 2;
const kernelNames = ["filtering", "aggregation", "geometry"];

export async function environmentMetadata(config, adapters) {
  return {
    recorded_at: new Date().toISOString(),
    operating_system: await operatingSystem(),
    hardware: await hardware(),
    node: process.version,
    browsers: Object.fromEntries(
      adapters.map((adapter) => [adapter.name, adapter.version]),
    ),
    workload: {
      timetable: await fileIdentity(config.timetable),
      platform: await optionalFileIdentity(config.platform),
      overlay: await optionalFileIdentity(config.overlay),
      kernel_iterations: config.kernelIterations,
      interaction_layer_pattern: config.interactionLayerPattern,
    },
    configuration: {
      timetable: config.timetable,
      platform: config.platform,
      overlay: config.overlay,
      browsers: config.browsers,
      warmups: config.warmups,
      measured_runs: config.runs,
      session_attempts: config.sessionAttempts,
      kernel_iterations: config.kernelIterations,
      interaction_layer_pattern: config.interactionLayerPattern,
      baseline: config.baseline,
      max_regression_percent: config.maxRegressionPercent,
    },
  };
}

export async function writeResults(config, metadata, samples) {
  const summary = summarizeSamples(samples);
  const validation = validateChecksums(samples);
  const baseline = config.baseline ? await readBaseline(config.baseline) : null;
  const regression = compareBaseline(
    summary,
    metadata,
    baseline,
    config.maxRegressionPercent,
  );
  const result = combineResults(validation, regression);
  const raw = {
    schema_version: schemaVersion,
    metadata,
    samples,
    summary,
    validation,
    regression,
  };

  await mkdir(config.output, { recursive: true });
  await Promise.all([
    writeFile(
      path.join(config.output, "benchmark.json"),
      `${JSON.stringify(raw, null, 2)}\n`,
    ),
    writeFile(
      path.join(config.output, "benchmark.md"),
      markdown(metadata, summary, validation, regression),
    ),
  ]);
  return result;
}

export function summarizeSamples(samples) {
  const values = {};
  for (const sample of samples.filter((sample) => !sample.warmup)) {
    values[sample.browser] ||= {};
    for (const [metric, value] of Object.entries(sample.metrics)) {
      values[sample.browser][metric] ||= [];
      values[sample.browser][metric].push(value);
    }
  }

  return Object.fromEntries(
    Object.entries(values).map(([browser, metrics]) => [
      browser,
      Object.fromEntries(
        Object.entries(metrics).map(([metric, samples]) => [
          metric,
          median(samples),
        ]),
      ),
    ]),
  );
}

export function compareBaseline(
  summary,
  metadata,
  baseline,
  maxRegressionPercent,
) {
  if (!baseline) {
    return {
      enabled: false,
      passed: true,
      max_regression_percent: maxRegressionPercent,
      comparisons: {},
      warnings: [],
      failures: [],
    };
  }

  const warnings = environmentWarnings(metadata, baseline.metadata);
  const failures = workloadFailures(
    metadata.workload,
    baseline.metadata?.workload,
  );
  const comparisons = {};
  if (failures.length > 0) {
    return {
      enabled: true,
      passed: false,
      max_regression_percent: maxRegressionPercent,
      comparisons,
      warnings,
      failures,
    };
  }
  for (const [browser, currentMetrics] of Object.entries(summary)) {
    const baselineMetrics = baseline.summary[browser];
    if (!baselineMetrics) {
      failures.push(`Baseline has no '${browser}' browser results`);
      continue;
    }
    comparisons[browser] = compareMetrics(
      browser,
      currentMetrics,
      baselineMetrics,
      maxRegressionPercent,
      failures,
    );
  }

  return {
    enabled: true,
    passed: failures.length === 0,
    max_regression_percent: maxRegressionPercent,
    comparisons,
    warnings,
    failures,
  };
}

export function validateChecksums(samples) {
  const failures = [];
  for (const browser of new Set(samples.map((sample) => sample.browser))) {
    for (const kernel of kernelNames) {
      const values = new Set(
        samples
          .filter((sample) => sample.browser === browser)
          .map((sample) => sample.checksums[kernel]),
      );
      if (values.size !== 1 || !Number.isFinite([...values][0])) {
        failures.push(
          `${browser} ${kernel} checksum was not stable: ${[...values].join(", ")}`,
        );
      }
    }
  }
  return { passed: failures.length === 0, failures };
}

function workloadFailures(current, baseline) {
  if (!current || !baseline) {
    return ["Benchmark workload metadata is missing"];
  }
  const failures = [];
  for (const [name, label] of [
    ["timetable", "Timetable"],
    ["platform", "Platform"],
    ["overlay", "Overlay"],
    ["kernel_iterations", "Kernel iterations"],
    ["interaction_layer_pattern", "Interaction layer pattern"],
  ]) {
    if (JSON.stringify(current[name]) !== JSON.stringify(baseline[name])) {
      failures.push(`${label} differs from baseline`);
    }
  }
  return failures;
}

function compareMetrics(
  browser,
  currentMetrics,
  baselineMetrics,
  maxRegressionPercent,
  failures,
) {
  const comparisons = {};
  for (const [metric, label] of Object.entries(metricLabels)) {
    const current = currentMetrics[metric];
    const baseline = baselineMetrics[metric];
    if (!isTiming(current)) {
      failures.push(`Current ${browser} results have no valid ${label} metric`);
      continue;
    }
    if (!isTiming(baseline)) {
      failures.push(
        `Baseline ${browser} results have no valid ${label} metric`,
      );
      continue;
    }

    const changePercent = percentageChange(current, baseline);
    comparisons[metric] = {
      current_ms: current,
      baseline_ms: baseline,
      change_percent: changePercent,
    };
    if (changePercent === null && current > baseline) {
      failures.push(
        `${browser} ${label} increased from a zero-millisecond baseline to ${current.toFixed(2)} ms`,
      );
    } else if (changePercent > maxRegressionPercent) {
      failures.push(
        `${browser} ${label} regressed by ${changePercent.toFixed(1)}%; limit is ${maxRegressionPercent}%`,
      );
    }
  }
  return comparisons;
}

function combineResults(validation, regression) {
  const failures = [...validation.failures, ...regression.failures];
  return { passed: failures.length === 0, failures };
}

async function readBaseline(filename) {
  const baseline = JSON.parse(await readFile(filename, "utf8"));
  if (baseline.schema_version !== schemaVersion) {
    throw new Error(
      `Baseline schema version must be ${schemaVersion}, found '${baseline.schema_version}'`,
    );
  }
  if (!baseline.summary || typeof baseline.summary !== "object") {
    throw new Error("Baseline does not contain a benchmark summary");
  }
  return baseline;
}

function environmentWarnings(current, baseline = {}) {
  const warnings = [];
  compareEnvironment(
    warnings,
    "Operating system",
    current.operating_system,
    baseline.operating_system,
  );
  compareEnvironment(warnings, "Hardware", current.hardware, baseline.hardware);
  for (const browser of Object.keys(current.browsers || {})) {
    compareEnvironment(
      warnings,
      `${browser} version`,
      current.browsers[browser],
      baseline.browsers?.[browser],
    );
  }
  return warnings;
}

function compareEnvironment(warnings, name, current, baseline) {
  if (current !== baseline) {
    warnings.push(
      `${name} differs from baseline: current '${current ?? "unknown"}', baseline '${baseline ?? "unknown"}'`,
    );
  }
}

function markdown(metadata, summary, validation, regression) {
  const lines = [
    "# GWR visualisation browser benchmark",
    "",
    `Recorded: ${metadata.recorded_at}`,
    "",
    `Platform: ${metadata.operating_system}; ${metadata.hardware}`,
    "",
  ];
  for (const [browser, metrics] of Object.entries(summary)) {
    lines.push(`## ${browser} ${metadata.browsers[browser]}`, "");
    if (regression.enabled && regression.comparisons[browser]) {
      lines.push(
        "| Scenario | Rust/WASM median (ms) | Baseline (ms) | Change |",
        "| --- | ---: | ---: | ---: |",
      );
      for (const [metric, label] of Object.entries(metricLabels)) {
        const comparison = regression.comparisons[browser][metric];
        if (comparison) {
          lines.push(
            `| ${label} | ${formatTiming(metrics[metric])} | ${formatTiming(comparison.baseline_ms)} | ${formatChange(comparison.change_percent)} |`,
          );
        }
      }
    } else {
      lines.push("| Scenario | Rust/WASM median (ms) |", "| --- | ---: |");
      for (const [metric, label] of Object.entries(metricLabels)) {
        lines.push(`| ${label} | ${formatTiming(metrics[metric])} |`);
      }
    }
    lines.push("");
  }

  lines.push(
    "## Checksum validation",
    "",
    validation.passed ? "PASS" : "FAIL",
    "",
  );
  appendItems(lines, validation.failures);
  lines.push("## Performance regression", "");
  if (!regression.enabled) {
    lines.push("Not evaluated: no explicit baseline was supplied.", "");
  } else {
    lines.push(regression.passed ? "PASS" : "FAIL", "");
    appendItems(lines, regression.failures);
    if (regression.warnings.length) {
      lines.push("Environment warnings:", "");
      appendItems(lines, regression.warnings);
    }
  }
  return `${lines.join("\n")}\n`;
}

function appendItems(lines, items) {
  for (const item of items) {
    lines.push(`- ${item}`);
  }
  if (items.length) {
    lines.push("");
  }
}

function formatTiming(value) {
  return Number.isFinite(value) ? value.toFixed(2) : "missing";
}

function formatChange(value) {
  return value === null
    ? "n/a"
    : `${value >= 0 ? "+" : ""}${value.toFixed(1)}%`;
}

function isTiming(value) {
  return Number.isFinite(value) && value >= 0;
}

function percentageChange(current, baseline) {
  if (baseline === 0) {
    return current === 0 ? 0 : null;
  }
  return ((current - baseline) / baseline) * 100;
}

function median(values) {
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2
    ? sorted[middle]
    : (sorted[middle - 1] + sorted[middle]) / 2;
}

async function operatingSystem() {
  if (os.platform() !== "darwin") {
    return `${os.type()} ${os.release()}`;
  }
  const { stdout } = await execute("sw_vers", ["-productVersion"]);
  return `macOS ${stdout.trim()}`;
}

async function hardware() {
  let processor = os.cpus()[0]?.model || os.arch();
  if (os.platform() === "darwin") {
    try {
      processor =
        (
          await execute("sysctl", ["-n", "machdep.cpu.brand_string"])
        ).stdout.trim() || processor;
    } catch {
      // Apple Silicon may not expose machdep.cpu.brand_string.
    }
  }
  return `${processor}, ${os.cpus().length} logical CPUs, ${(os.totalmem() / 2 ** 30).toFixed(1)} GiB RAM`;
}

async function optionalFileIdentity(filename) {
  return filename ? fileIdentity(filename) : null;
}

async function fileIdentity(filename) {
  const contents = await readFile(filename);
  return {
    sha256: createHash("sha256").update(contents).digest("hex"),
  };
}
