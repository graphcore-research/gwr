// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

import { serveReport } from "../lib/report-server.mjs";

test("serveReport serves files from the report directory", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "gwr-report-server-"));
  const indexPath = path.join(root, "index.html");
  await writeFile(indexPath, '<script src="app.js"></script>\n');
  await writeFile(path.join(root, "app.js"), "window.ready = true;\n");

  const report = await serveReport(pathToFileURL(indexPath));
  try {
    const index = await fetch(report.url);
    assert.equal(index.status, 200);
    assert.equal(index.headers.get("content-type"), "text/html; charset=utf-8");
    assert.equal(await index.text(), '<script src="app.js"></script>\n');

    const script = await fetch(new URL("app.js", report.url));
    assert.equal(script.status, 200);
    assert.equal(
      script.headers.get("content-type"),
      "text/javascript; charset=utf-8",
    );
    assert.equal(await script.text(), "window.ready = true;\n");

    const missing = await fetch(new URL("missing.js", report.url));
    assert.equal(missing.status, 404);

    const reportUrl = new URL(report.url);
    const outside = await fetch(
      `${reportUrl.origin}/%2e%2e%2foutside-report-directory`,
    );
    assert.equal(outside.status, 403);
  } finally {
    await report.close();
    await rm(root, { recursive: true, force: true });
  }
});
