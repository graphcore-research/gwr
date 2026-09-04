// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import { execFile } from "node:child_process";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { promisify } from "node:util";

const execute = promisify(execFile);
const repository = fileURLToPath(new URL("../../..", import.meta.url));

export async function prepareReports(config) {
  const root = await mkdtemp(
    path.join(os.tmpdir(), "gwr-visualisation-browser-"),
  );
  const base = path.join(root, "wasm-base");
  await generateReport(config, base);
  const regression = path.join(root, "wasm-regression");
  const regressionTimetable = path.join(root, "regression-timetable.yaml");
  const regressionPlatform = path.join(root, "regression-platform.yaml");
  await writeFile(regressionTimetable, regressionTimetableYaml);
  await writeFile(regressionPlatform, regressionPlatformYaml);
  await generateReport(
    {
      timetable: regressionTimetable,
      platform: regressionPlatform,
    },
    regression,
  );
  let sample = 0;

  async function copyReport(source, suffix) {
    const target = path.join(
      root,
      `sample-${String(sample++).padStart(4, "0")}-${suffix}`,
    );
    await cp(source, target, { recursive: true });
    return target;
  }

  async function stageReport(source, suffix) {
    const target = await copyReport(source, suffix);
    const bootstrapPath = path.join(target, "bootstrap.js");
    const bootstrap = await readFile(bootstrapPath, "utf8");
    const delayedBootstrap = bootstrap.replace(
      "const tensors = await decompressGzip(decodeBase64(payload.tensors));",
      "await new Promise((resolve) => { window.GWR_CONTINUE_LOADING = resolve; });\n    const tensors = await decompressGzip(decodeBase64(payload.tensors));",
    );
    if (delayedBootstrap === bootstrap) {
      throw new Error("Unable to add the staged-loading test delay");
    }
    await writeFile(bootstrapPath, delayedBootstrap);
    return target;
  }

  return {
    root,
    async sample() {
      return reportUrl(await copyReport(base, "wasm"));
    },
    async regressionSample() {
      return reportUrl(await copyReport(regression, "regression"));
    },
    async stagedSample() {
      return reportUrl(await stageReport(base, "staged"));
    },
    async stagedBrokenSample() {
      const target = await stageReport(base, "staged-broken");
      const payloadPath = path.join(target, "payload.js");
      const payload = await readFile(payloadPath, "utf8");
      const brokenPayload = payload.replace(/tensors:"[^"]*"/, 'tensors:"!"');
      if (brokenPayload === payload) {
        throw new Error("Unable to corrupt the staged tensor payload");
      }
      await writeFile(payloadPath, brokenPayload);
      return reportUrl(target);
    },
    async brokenSample() {
      const target = await copyReport(base, "broken");
      await writeFile(
        path.join(target, "payload.js"),
        "window.GWR_VISUALISATION_PAYLOAD=null;\n",
      );
      return reportUrl(target);
    },
    async close() {
      if (!config.keepReports) {
        await rm(root, { recursive: true, force: true });
      }
    },
  };
}

const regressionTimetableYaml = `
nodes:
  - id: large_shape
    kind: tensor
    config:
      addr: 0
      dtype: int4
      shape: [4294967296]
  - id: final_tensor
    kind: tensor
    config:
      addr: 18446744073709551614
      dtype: int8
      shape: [2]
  - id: consumer
    kind: compute
    op:
      custom:
        name: consumer
        machine_ops: {}
    pe: final_pe
    input_views: [null]
    output_views: []
edges:
  - { from: final_tensor, to: consumer, kind: data }
`;

const regressionPlatformYaml = `
memory_maps:
  - name: final_map
    devices:
      - name: final_memory
processing_elements:
  - name: final_pe
    memory_map: final_map
    config:
memories:
  - name: final_memory
    kind: hbm
    base_address: 18446744073709551614
    config:
      capacity_bytes: 2
`;

async function generateReport(config, destination) {
  const arguments_ = [
    "run",
    "--release",
    "-p",
    "gwr-visualisation",
    "--",
    "--timetable",
    config.timetable,
    "--out",
    destination,
  ];
  addOptional(arguments_, "--platform", config.platform);
  addOptional(arguments_, "--overlay", config.overlay);
  await execute("cargo", arguments_, {
    cwd: repository,
    maxBuffer: 16 * 1024 * 1024,
  });
}

function reportUrl(directory) {
  return pathToFileURL(path.join(directory, "index.html")).href;
}

function addOptional(arguments_, flag, value) {
  if (value) {
    arguments_.push(flag, value);
  }
}
