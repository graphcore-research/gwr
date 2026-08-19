// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import os from "node:os";
import path from "node:path";

export function readConfig(argv) {
  const values = parseArguments(argv);
  if (!values.timetable) {
    throw new Error("Pass --timetable /absolute/path/to/timetable.yaml");
  }
  return {
    timetable: path.resolve(values.timetable),
    platform: values.platform ? path.resolve(values.platform) : null,
    overlay: values.overlay ? path.resolve(values.overlay) : null,
    output: path.resolve(values.output || "benchmark-results"),
    browsers: (values.browsers || "chromium,safari").split(","),
    warmups: positiveInteger(values.warmups, 2),
    runs: positiveInteger(values.runs, 10),
    sessionAttempts: positiveInteger(values["session-attempts"], 3),
    kernelIterations: positiveInteger(values["kernel-iterations"], 3),
    interactionLayerPattern:
      values["interaction-layer-pattern"] ||
      "^layer ([1-9]|[1-5][0-9]|6[0-4])$",
    baseline: values.baseline ? path.resolve(values.baseline) : null,
    maxRegressionPercent: nonNegativeNumber(
      values["max-regression-percent"],
      10,
    ),
    keepReports: values["keep-reports"] === true,
    chromiumExecutable: values.chromium || defaultChromiumExecutable(),
  };
}

function defaultChromiumExecutable() {
  if (process.env.CHROME_PATH) {
    return process.env.CHROME_PATH;
  }
  return os.platform() === "darwin"
    ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
    : "/usr/bin/google-chrome";
}

function parseArguments(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (!argument.startsWith("--")) {
      throw new Error(`Unexpected argument: ${argument}`);
    }
    const name = argument.slice(2);
    if (name === "keep-reports") {
      values[name] = true;
      continue;
    }
    const value = argv[++index];
    if (!value || value.startsWith("--")) {
      throw new Error(`Missing value for --${name}`);
    }
    values[name] = value;
  }
  return values;
}

function nonNegativeNumber(value, fallback) {
  const parsed = value === undefined ? fallback : Number(value);
  if (!Number.isFinite(parsed) || parsed < 0) {
    throw new Error(`Expected a non-negative number, found '${value}'`);
  }
  return parsed;
}

function positiveInteger(value, fallback) {
  const parsed = value === undefined ? fallback : Number(value);
  if (!Number.isInteger(parsed) || parsed < 1) {
    throw new Error(`Expected a positive integer, found '${value}'`);
  }
  return parsed;
}
