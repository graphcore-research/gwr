// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import { browserAdapters } from "./lib/adapters.mjs";
import { mkdir } from "node:fs/promises";
import { readConfig } from "./lib/config.mjs";
import { prepareReports } from "./lib/reports.mjs";
import { environmentMetadata, writeResults } from "./lib/results.mjs";
import { runBenchmarkScenario } from "./lib/scenarios.mjs";

const config = readConfig(process.argv.slice(2));
const reports = await prepareReports(config);
const adapters = browserAdapters(config);
const samples = [];

await mkdir(config.output, { recursive: true });

try {
  for (const adapter of adapters) {
    for (let round = 0; round < config.warmups + config.runs; round += 1) {
      const { attempt, result, url } = await runSample(
        adapter,
        reports,
        config,
      );
      samples.push({
        browser: adapter.name,
        implementation: "wasm",
        round,
        session_attempt: attempt,
        warmup: round < config.warmups,
        report_url: url,
        ...result,
      });
      process.stdout.write(
        `${adapter.name} Rust/WASM ${round < config.warmups ? "warm-up" : "run"} ${round + 1}/${config.warmups + config.runs}\n`,
      );
    }
  }
  const metadata = await environmentMetadata(config, adapters);
  const result = await writeResults(config, metadata, samples);
  if (!result.passed) {
    throw new Error(
      `Benchmark validation failed:\n${result.failures.join("\n")}`,
    );
  }
} finally {
  await reports.close();
}

async function runSample(adapter, reports, config) {
  let lastError;
  for (let attempt = 1; attempt <= config.sessionAttempts; attempt += 1) {
    const url = await reports.sample();
    try {
      const result = await adapter.withSession(
        url,
        (session) =>
          runBenchmarkScenario(
            session,
            config.kernelIterations,
            config.interactionLayerPattern,
            (scenario, milliseconds) =>
              process.stdout.write(
                `  ${adapter.name} Rust/WASM ${scenario}: ${milliseconds.toFixed(2)} ms\n`,
              ),
          ),
        {
          failureEvidence: {
            directory: config.output,
            name: `${adapter.name}-attempt-${attempt}`,
          },
        },
      );
      return { attempt, result, url };
    } catch (error) {
      lastError = error;
      process.stderr.write(
        `${adapter.name} Rust/WASM session attempt ${attempt}/${config.sessionAttempts} failed: ${error.message}\n`,
      );
      if (attempt < config.sessionAttempts) {
        await new Promise((resolve) => setTimeout(resolve, 1_000));
      }
    }
  }
  throw lastError;
}
