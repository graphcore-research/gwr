// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import { execFile } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";

import { metricLabels } from "./scenarios.mjs";

const execute = promisify(execFile);

export async function environmentMetadata(config, adapters) {
  return {
    recorded_at: new Date().toISOString(),
    operating_system: await operatingSystem(),
    hardware: await hardware(),
    node: process.version,
    browsers: Object.fromEntries(
      adapters.map((adapter) => [adapter.name, adapter.version]),
    ),
    configuration: {
      timetable: config.timetable,
      platform: config.platform,
      overlay: config.overlay,
      warmups: config.warmups,
      measured_runs: config.runs,
      session_attempts: config.sessionAttempts,
      kernel_iterations: config.kernelIterations,
      interaction_layer_pattern: config.interactionLayerPattern,
    },
  };
}

export async function writeResults(config, metadata, samples) {
  const summary = summarize(samples);
  const validation = validateChecksums(samples);
  const raw = { metadata, samples, summary, validation };
  await mkdir(config.output, { recursive: true });
  await Promise.all([
    writeFile(
      path.join(config.output, "benchmark.json"),
      `${JSON.stringify(raw, null, 2)}\n`,
    ),
    writeFile(
      path.join(config.output, "benchmark.md"),
      markdown(metadata, summary, validation),
    ),
  ]);
  return validation;
}

function summarize(samples) {
  const result = {};
  for (const sample of samples.filter((sample) => !sample.warmup)) {
    result[sample.browser] ||= {};
    for (const [metric, value] of Object.entries(sample.metrics)) {
      result[sample.browser][metric] ||= [];
      result[sample.browser][metric].push(value);
    }
  }
  for (const browser of Object.values(result)) {
    for (const [metric, values] of Object.entries(browser)) {
      browser[metric] = median(values);
    }
  }
  return result;
}

function validateChecksums(samples) {
  const failures = [];
  for (const browser of new Set(samples.map((sample) => sample.browser))) {
    for (const kernel of ["filtering", "aggregation", "geometry"]) {
      const values = new Set(
        samples
          .filter((sample) => sample.browser === browser)
          .map((sample) => sample.checksums[kernel]),
      );
      if (values.size !== 1) {
        failures.push(
          `${browser} ${kernel} checksums varied: ${[...values].join(", ")}`,
        );
      }
    }
  }
  return { passed: failures.length === 0, failures };
}

function markdown(metadata, summary, validation) {
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
    lines.push(
      "| Scenario | JavaScript median (ms) |",
      "| --- | ---: |",
    );
    for (const [metric, label] of Object.entries(metricLabels)) {
      lines.push(`| ${label} | ${metrics[metric].toFixed(2)} |`);
    }
    lines.push("");
  }
  lines.push(
    "## Checksum validation",
    "",
    validation.passed ? "PASS" : "FAIL",
    "",
  );
  for (const failure of validation.failures) {
    lines.push(`- ${failure}`);
  }
  if (validation.failures.length) {
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
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
      processor = (
        await execute("sysctl", ["-n", "machdep.cpu.brand_string"])
      ).stdout.trim();
    } catch {
      // Apple Silicon may not expose machdep.cpu.brand_string.
    }
  }
  return `${processor}, ${os.cpus().length} logical CPUs, ${(
    os.totalmem() /
    2 ** 30
  ).toFixed(1)} GiB RAM`;
}
