// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import { execFile } from "node:child_process";
import { cp, mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { promisify } from "node:util";

const execute = promisify(execFile);
const repository = fileURLToPath(new URL("../../..", import.meta.url));

export async function prepareReports(config) {
  const root = await mkdtemp(
    path.join(os.tmpdir(), "gwr-visualisation-benchmark-"),
  );
  const base = path.join(root, "javascript-base");
  await generateReport(config, base);
  let sample = 0;

  return {
    root,
    async sample() {
      const target = path.join(
        root,
        `sample-${String(sample++).padStart(4, "0")}-javascript`,
      );
      await cp(base, target, { recursive: true });
      return pathToFileURL(path.join(target, "index.html")).href;
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

function addOptional(arguments_, flag, value) {
  if (value) {
    arguments_.push(flag, value);
  }
}
