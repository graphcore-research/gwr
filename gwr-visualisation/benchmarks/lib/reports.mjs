// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import { execFile } from "node:child_process";
import { cp, mkdtemp, rm, writeFile } from "node:fs/promises";
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
  let sample = 0;

  async function copyReport(suffix) {
    const target = path.join(
      root,
      `sample-${String(sample++).padStart(4, "0")}-${suffix}`,
    );
    await cp(base, target, { recursive: true });
    return target;
  }

  return {
    root,
    async sample() {
      return reportUrl(await copyReport("wasm"));
    },
    async brokenSample() {
      const target = await copyReport("broken");
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
