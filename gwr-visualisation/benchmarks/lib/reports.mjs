// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import { execFile } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, cp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { promisify } from "node:util";

const execute = promisify(execFile);
const repository = fileURLToPath(new URL("../../..", import.meta.url));
const crate = path.join(repository, "gwr-visualisation");
const legacyAssets = path.join(crate, "benchmarks", "legacy", "assets");

export async function prepareReports(config) {
  const root = await mkdtemp(path.join(os.tmpdir(), "gwr-visualisation-benchmark-"));
  const wasm = path.join(root, "wasm-base");
  await generateWasmReport(config, wasm);
  const legacy = path.join(root, "javascript-base");
  await generateLegacyReport(wasm, legacy);
  let sample = 0;
  return {
    root,
    async sample(implementation) {
      const target = path.join(root, `sample-${String(sample++).padStart(4, "0")}-${implementation}`);
      await cp(implementation === "wasm" ? wasm : legacy, target, { recursive: true });
      return pathToFileURL(path.join(target, "index.html")).href;
    },
    async close() {
      if (!config.keepReports) {
        await rm(root, { recursive: true, force: true });
      }
    },
  };
}

async function generateWasmReport(config, destination) {
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
  await execute("cargo", arguments_, { cwd: repository, maxBuffer: 16 * 1024 * 1024 });
}

async function generateLegacyReport(wasm, destination) {
  await mkdir(destination, { recursive: true });
  const [index, style, data] = await Promise.all([
    readFile(path.join(wasm, "index.html"), "utf8"),
    readFile(path.join(wasm, "style.css")),
    readFile(path.join(wasm, "data.json"), "utf8"),
  ]);
  await Promise.all([
    writeFile(path.join(destination, "index.html"), legacyIndex(index)),
    writeFile(path.join(destination, "style.css"), style),
    writeFile(path.join(destination, "data.json"), data),
    writeFile(
      path.join(destination, "data.js"),
      `window.GWR_VISUALISATION_DATA=${JSON.stringify(JSON.parse(data))};\n`,
    ),
  ]);
  for (const name of legacyScriptNames()) {
    await cp(path.join(legacyAssets, name), path.join(destination, name));
  }
}

function legacyIndex(index) {
  const production = [
    '<script src="payload.js"></script>',
    '<script src="gwr_visualisation.js"></script>',
    '<script src="bootstrap.js"></script>',
  ].join("\n    ");
  const scripts = ["data.js", ...legacyScriptNames()]
    .map((name) => `<script src="${name}"></script>`)
    .join("\n    ");
  if (!index.includes(production)) {
    throw new Error("Production script block has changed; update the legacy fixture generator");
  }
  return index.replace(production, scripts);
}

function legacyScriptNames() {
  return [
    "view-model.js",
    "core.js",
    "filters.js",
    "pe-grid.js",
    "timetable.js",
    "tensors.js",
    "memory.js",
    "relationships.js",
    "workspace.js",
    "app.js",
    "benchmark-hooks.js",
  ];
}

function addOptional(arguments_, flag, value) {
  if (value) {
    arguments_.push(flag, value);
  }
}
