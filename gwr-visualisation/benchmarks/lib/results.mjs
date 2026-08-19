// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import { execFile } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";

import { interactionMetrics, metricLabels } from "./scenarios.mjs";

const execute = promisify(execFile);

export async function environmentMetadata(config, adapters) {
  return {
    recorded_at: new Date().toISOString(),
    operating_system: await operatingSystem(),
    hardware: await hardware(),
    node: process.version,
    browsers: Object.fromEntries(adapters.map((adapter) => [adapter.name, adapter.version])),
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
  const gates = evaluateGates(summary, samples);
  const raw = { metadata, samples, summary, gates };
  await mkdir(config.output, { recursive: true });
  await Promise.all([
    writeFile(path.join(config.output, "benchmark.json"), `${JSON.stringify(raw, null, 2)}\n`),
    writeFile(path.join(config.output, "benchmark.md"), markdown(metadata, summary, gates)),
  ]);
  return gates;
}

function summarize(samples) {
  const result = {};
  for (const sample of samples.filter((sample) => !sample.warmup)) {
    result[sample.browser] ||= {};
    result[sample.browser][sample.implementation] ||= {};
    for (const [metric, value] of Object.entries(sample.metrics)) {
      result[sample.browser][sample.implementation][metric] ||= [];
      result[sample.browser][sample.implementation][metric].push(value);
    }
  }
  for (const browser of Object.values(result)) {
    for (const implementation of Object.values(browser)) {
      for (const [metric, values] of Object.entries(implementation)) {
        implementation[metric] = median(values);
      }
    }
  }
  return result;
}

function evaluateGates(summary, samples) {
  const failures = checksumFailures(samples);
  for (const [browser, implementations] of Object.entries(summary)) {
    const javascript = implementations.javascript;
    const wasm = implementations.wasm;
    const startupSpeedup = javascript.cold_startup_ms / wasm.cold_startup_ms;
    if (startupSpeedup < 2) {
      failures.push(`${browser} cold startup speedup was ${startupSpeedup.toFixed(2)}×; expected at least 2×`);
    }
    for (const metric of interactionMetrics) {
      const ratio = wasm[metric] / javascript[metric];
      if (ratio > 1.1) {
        failures.push(`${browser} ${metricLabels[metric]} was ${((ratio - 1) * 100).toFixed(1)}% slower; limit is 10%`);
      }
    }
  }
  return { passed: failures.length === 0, failures };
}

function checksumFailures(samples) {
  const failures = [];
  const browsers = new Set(samples.map((sample) => sample.browser));
  for (const browser of browsers) {
    for (const kernel of ["filtering", "aggregation", "geometry"]) {
      const values = Object.fromEntries(
        ["javascript", "wasm"].map((implementation) => [
          implementation,
          new Set(
            samples
              .filter(
                (sample) =>
                  sample.browser === browser &&
                  sample.implementation === implementation,
              )
              .map((sample) => sample.checksums[kernel]),
          ),
        ]),
      );
      const javascript = [...values.javascript];
      const wasm = [...values.wasm];
      if (
        javascript.length !== 1 ||
        wasm.length !== 1 ||
        javascript[0] !== wasm[0]
      ) {
        failures.push(
          `${browser} ${kernel} checksums differed: JavaScript ${javascript.join(", ")}; WASM ${wasm.join(", ")}`,
        );
      }
    }
  }
  return failures;
}

function markdown(metadata, summary, gates) {
  const lines = [
    "# GWR visualisation browser benchmark",
    "",
    `Recorded: ${metadata.recorded_at}`,
    "",
    `Platform: ${metadata.operating_system}; ${metadata.hardware}`,
    "",
  ];
  for (const [browser, implementations] of Object.entries(summary)) {
    lines.push(`## ${browser} ${metadata.browsers[browser]}`, "");
    lines.push("| Scenario | JavaScript median (ms) | Rust/WASM median (ms) | Speedup |", "| --- | ---: | ---: | ---: |");
    for (const metric of Object.keys(metricLabels)) {
      const javascript = implementations.javascript[metric];
      const wasm = implementations.wasm[metric];
      lines.push(`| ${metricLabels[metric]} | ${javascript.toFixed(2)} | ${wasm.toFixed(2)} | ${(javascript / wasm).toFixed(2)}× |`);
    }
    lines.push("");
  }
  lines.push("## Performance gates", "", gates.passed ? "PASS" : "FAIL", "");
  for (const failure of gates.failures) {
    lines.push(`- ${failure}`);
  }
  if (gates.failures.length) {
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
}

function median(values) {
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
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
      processor = (await execute("sysctl", ["-n", "machdep.cpu.brand_string"])).stdout.trim() || processor;
    } catch {
      // Apple Silicon may not expose machdep.cpu.brand_string.
    }
  }
  return `${processor}, ${os.cpus().length} logical CPUs, ${(os.totalmem() / 2 ** 30).toFixed(1)} GiB RAM`;
}
